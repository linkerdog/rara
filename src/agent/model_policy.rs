use rara_agent::{ModelRequest, ModelTurnEvent, ModelTurnPolicy, StreamEvidence};
use rara_agent_trace::TraceModelStatus;
use rara_core::llm::contracts::ModelRequestFingerprint;
use rara_core::llm::types::LlmResponse;
use rara_observability::InferenceCall;

use super::*;

pub(super) struct NativeModelPolicy<'a, 'request, F> {
    pub(super) agent: &'a mut Agent,
    pub(super) report: &'a mut F,
    pub(super) output_mode: AgentOutputMode,
    pub(super) request: &'a ModelRequest<'request>,
    pub(super) model_label: String,
    pub(super) request_fingerprint: Option<ModelRequestFingerprint>,
    pub(super) request_started_at: std::time::Instant,
    pub(super) inference_call: Option<InferenceCall>,
    pub(super) plan_updated: bool,
    pub(super) malformed_proposed_plan: bool,
    pub(super) continue_inspection: bool,
}

impl<F: FnMut(AgentEvent) + Send> ModelTurnPolicy for NativeModelPolicy<'_, '_, F> {
    fn event(&mut self, event: ModelTurnEvent) {
        let event = match event {
            ModelTurnEvent::Stream(LlmStreamEvent::TextDelta(delta)) => {
                AgentEvent::AssistantDelta(delta)
            }
            ModelTurnEvent::Stream(LlmStreamEvent::ReasoningDelta(delta)) => {
                AgentEvent::AssistantThinkingDelta(delta)
            }
            ModelTurnEvent::AssistantText(text) => AgentEvent::AssistantText(text),
            ModelTurnEvent::ToolUse(call) => AgentEvent::ToolUse {
                call_id: call.id,
                name: call.name,
                input: call.input,
            },
        };
        (self.report)(event);
    }

    fn observe_response(&mut self, response: &Result<LlmResponse>) {
        if let Some(call) = self.inference_call.take() {
            call.finish(response);
        }
        let duration_ms = self
            .request_started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let Ok(response) = response else {
            self.agent.record_agent_trace_model_finished(
                self.model_label.clone(),
                duration_ms,
                TraceModelStatus::Failed,
                None,
                None,
            );
            return;
        };
        self.agent.capture_summary_prefix(
            self.request.messages,
            self.request.tools,
            &self.request.metadata,
        );
        let output_tokens = response
            .usage
            .as_ref()
            .map(|usage| usage.output_tokens)
            .unwrap_or(0);
        (self.report)(AgentEvent::ModelResponse {
            model: self.model_label.clone(),
            output_tokens,
            finish_reason: response.stop_reason.clone(),
        });
        self.agent.record_agent_trace_model_finished(
            self.model_label.clone(),
            duration_ms,
            TraceModelStatus::Succeeded,
            response.stop_reason.clone(),
            response.usage.as_ref(),
        );
        self.agent
            .last_query_report
            .model_turns
            .push(ModelTurnReport {
                model: self.model_label.clone(),
                duration_ms,
                finish_reason: response.stop_reason.clone(),
                usage: response
                    .usage
                    .as_ref()
                    .map(ModelTokenUsage::from_provider_usage),
                request_fingerprint: self.request_fingerprint.take(),
            });
        if let Some(usage) = &response.usage {
            self.agent.total_input_tokens += usage.input_tokens;
            self.agent.total_output_tokens += usage.output_tokens;
            self.agent.total_cache_hit_tokens += usage.cache_hit_tokens;
            self.agent.total_cache_miss_tokens += usage.cache_miss_tokens;
            // Anchor the estimate before this response is appended to history.
            self.agent.record_actual_prompt_tokens(usage);
        }
    }

    #[expect(
        clippy::print_stdout,
        reason = "Explicit Terminal output mode; TUI and protocol callers use Silent."
    )]
    fn prepare_text(&mut self, text: &str, stream: StreamEvidence) -> Result<String> {
        let (clean_text, block_requests_continue) =
            planning::strip_continue_inspection_control(text);
        self.continue_inspection |= block_requests_continue;
        let clean_text = scrub_internal_control_tokens(&clean_text);
        if !clean_text.trim().is_empty() {
            if !stream.text_delta {
                self.event(ModelTurnEvent::AssistantText(clean_text.clone()));
            }
            if matches!(self.agent.execution_mode, AgentExecutionMode::Plan) {
                self.malformed_proposed_plan |=
                    planning::has_unclosed_proposed_plan_block(&clean_text);
                if self.agent.capture_plan_from_text(&clean_text)? {
                    self.plan_updated = true;
                    (self.report)(AgentEvent::PlanUpdated {
                        steps: self.agent.current_plan.clone(),
                        explanation: self.agent.plan_explanation.clone(),
                    });
                }
            }
            if matches!(self.output_mode, AgentOutputMode::Terminal) {
                println!("Agent: {}", clean_text);
            }
        }
        Ok(clean_text)
    }

    fn prepare_tool_input(&mut self, call: &ToolCall) -> Result<Value> {
        if matches!(self.agent.execution_mode, AgentExecutionMode::Plan)
            && call.name == EXIT_PLAN_MODE_TOOL_NAME
            && !self.plan_updated
            && let Some((steps, explanation)) = planning::parse_exit_plan_tool_input(&call.input)
        {
            self.agent.current_plan = steps;
            self.agent.plan_explanation = explanation;
            self.plan_updated = true;
            (self.report)(AgentEvent::PlanUpdated {
                steps: self.agent.current_plan.clone(),
                explanation: self.agent.plan_explanation.clone(),
            });
        }
        Ok(match self.agent.hook_runtime.as_ref() {
            Some(runtime) => runtime.modify_tool_input(&call.name, call.input.clone()),
            None => call.input.clone(),
        })
    }

    fn finish_response(&mut self) -> Result<()> {
        if matches!(self.agent.execution_mode, AgentExecutionMode::Plan) && self.plan_updated {
            self.agent.save_current_plan_file()?;
        }
        Ok(())
    }
}
