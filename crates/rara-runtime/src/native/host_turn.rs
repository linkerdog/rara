use std::time::Instant;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use rara_agent::{
    Continuation, ContinuationContext, IterationBudget, LoopEffects, LoopEnd, LoopProgress,
    ModelObservation, ModelRequest, ModelTurnEvent, ModelTurnPolicy, StopHookContext,
    StopHookOutcome, TextContinuation, ToolAdmission, ToolBatchEffects, ToolBatchOutcome, ToolCall,
    ToolReply, execute_loop, execute_model_turn, execute_tool_batch, execute_tool_call,
};
use rara_core::llm::contracts::ModelRequestFingerprint;
use rara_core::{
    llm::backend::LlmTurnMetadata,
    llm::contracts::LlmStreamEvent,
    llm::types::{LlmResponse, Message},
    observation::{ModelTokenUsage, ModelTurnReport, QueryReport},
    tool::{ToolCallContext, ToolError},
};
use rara_observability::{InferenceCall, InferencePurpose};
use serde_json::{Value, json};

use super::host_driver::{HostEvents, HostState};
use crate::{AssistantEvent, RuntimeEvent, RuntimeTurnId, SessionTurn, ToolEvent, TurnContext};

pub(crate) struct HostTurn {
    pub state: HostState,
    pub report: QueryReport,
    events: HostEvents,
    context: TurnContext,
    prompt: Option<String>,
    assistant: Option<Message>,
    calls: Vec<ToolCall>,
}

impl HostTurn {
    pub fn new(state: HostState, events: HostEvents, prompt: String, context: TurnContext) -> Self {
        Self {
            state,
            events,
            context,
            prompt: Some(prompt),
            report: QueryReport::default(),
            assistant: None,
            calls: Vec::new(),
        }
    }

    // Commit each completed reply before another call can fail or suspend.
    fn record_reply(&mut self, call: &ToolCall, reply: &ToolReply) {
        self.state
            .transcript
            .push(reply.clone().into_message(&call.id));
        self.events.publish(
            Some(&self.context.turn_id),
            RuntimeEvent::Tool(ToolEvent::Result {
                call_id: call.id.clone(),
                name: call.name.clone(),
                content: reply.content.clone(),
                is_error: reply.is_error,
            }),
        );
    }
}

#[async_trait]
impl SessionTurn for HostTurn {
    async fn run(&mut self) -> Result<()> {
        let prompt = self
            .prompt
            .take()
            .context("host session turn executed twice")?;
        self.state.transcript = rara_agent::repair_tool_result_history(&self.state.transcript);
        self.state.transcript.push(Message {
            role: "user".into(),
            content: json!([{"type": "text", "text": prompt}]),
        });
        execute_loop(self, &mut LoopProgress::default()).await?;
        Ok(())
    }
}

struct HostModelPolicy<'a> {
    events: &'a HostEvents,
    turn_id: &'a RuntimeTurnId,
    report: &'a mut QueryReport,
    model: String,
    started: Instant,
    call: Option<InferenceCall>,
    fingerprint: Option<ModelRequestFingerprint>,
}

impl ModelTurnPolicy for HostModelPolicy<'_> {
    fn event(&mut self, event: ModelTurnEvent) {
        let event = match event {
            ModelTurnEvent::Stream(LlmStreamEvent::TextDelta(text)) => {
                RuntimeEvent::Assistant(AssistantEvent::Delta(text))
            }
            ModelTurnEvent::Stream(LlmStreamEvent::ReasoningDelta(text)) => {
                RuntimeEvent::Assistant(AssistantEvent::ThinkingDelta(text))
            }
            ModelTurnEvent::AssistantText(text) => {
                RuntimeEvent::Assistant(AssistantEvent::Text(text))
            }
            ModelTurnEvent::ToolUse(call) => RuntimeEvent::Tool(ToolEvent::Use {
                call_id: call.id,
                name: call.name,
                input: call.input,
            }),
        };
        self.events.publish(Some(self.turn_id), event);
    }

    fn observe_response(&mut self, response: &Result<LlmResponse>) {
        if let Some(call) = self.call.take() {
            call.finish(response);
        }
        if let Ok(response) = response {
            self.report.model_turns.push(ModelTurnReport {
                model: self.model.clone(),
                duration_ms: self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                finish_reason: response.stop_reason.clone(),
                usage: response
                    .usage
                    .as_ref()
                    .map(ModelTokenUsage::from_provider_usage),
                request_fingerprint: self.fingerprint.take(),
            });
        }
    }
}

#[async_trait]
impl LoopEffects for HostTurn {
    fn budget(&self) -> IterationBudget {
        IterationBudget {
            max_turns: self.state.max_turns,
            ..Default::default()
        }
    }

    async fn request_model(&mut self, _: LoopProgress) -> Result<ModelObservation> {
        let mut messages = Vec::with_capacity(self.state.transcript.len() + 1);
        if let Some(prompt) = &self.state.system_prompt {
            messages.push(Message {
                role: "system".into(),
                content: Value::String(prompt.clone()),
            });
        }
        messages.extend_from_slice(&self.state.transcript);
        let tools = self.state.tools.get_schemas();
        let call = self
            .context
            .inference_agent
            .start_call(InferencePurpose::Main);
        let request = ModelRequest {
            messages: &messages,
            tools: &tools,
            metadata: LlmTurnMetadata::execute()
                .with_cancellation(self.context.cancellation.clone())
                .with_inference(call.context()),
        };
        let mut policy = HostModelPolicy {
            events: &self.events,
            turn_id: &self.context.turn_id,
            report: &mut self.report,
            model: self
                .state
                .backend
                .model_label()
                .unwrap_or_else(|| "host".into()),
            started: Instant::now(),
            call: Some(call),
            fingerprint: self.state.backend.request_cache_fingerprint(
                request.messages,
                request.tools,
                &request.metadata,
            ),
        };
        let output = execute_model_turn(self.state.backend.as_ref(), &request, &mut policy).await?;
        self.assistant = output.assistant_message;
        self.calls = output.tool_calls;
        Ok(ModelObservation {
            tool_call_count: self.calls.len(),
            response: output.response,
            ..Default::default()
        })
    }

    async fn record_assistant(&mut self, _: LoopProgress) -> Result<ContinuationContext> {
        if let Some(message) = self.assistant.take() {
            self.state.transcript.push(message);
        }
        Ok(ContinuationContext::default())
    }

    async fn continue_turn(&mut self, continuation: Continuation, _: LoopProgress) -> Result<()> {
        match continuation {
            Continuation::Automatic(
                TextContinuation::ExecuteReasoningOnly | TextContinuation::ExecuteNeedsInspection,
            ) => {
                self.state.transcript.push(Message { role: "user".into(), content: json!([{"type": "text", "text": "Continue the task. Return the result or call a tool to make progress."}]) });
                Ok(())
            }
            Continuation::PlanExitRepair(_)
            | Continuation::StopHookBlocked
            | Continuation::Automatic(
                TextContinuation::PlanNeedsEvidence | TextContinuation::PlanReasoningOnly,
            ) => bail!("host execution received an unsupported continuation: {continuation:?}"),
        }
    }

    async fn run_stop_hooks(
        &mut self,
        _: StopHookContext,
        _: LoopProgress,
    ) -> Result<StopHookOutcome> {
        Ok(StopHookOutcome::AllowCompletion)
    }

    async fn run_tools(&mut self, _: LoopProgress) -> Result<ToolBatchOutcome> {
        let output = execute_tool_batch(std::mem::take(&mut self.calls), self).await?;
        Ok(output.outcome)
    }

    async fn commit_tool_results(&mut self, _: LoopProgress) -> Result<()> {
        // Replies were committed by result policy, including partial batches.
        Ok(())
    }

    async fn finalize(&mut self, end: LoopEnd, _: LoopProgress) -> Result<()> {
        match end {
            LoopEnd::ResponseComplete => {}
            LoopEnd::MaxTurnsReached { limit } => self.events.publish(
                Some(&self.context.turn_id),
                RuntimeEvent::Status(format!("Agent reached max-turns limit ({limit})")),
            ),
            LoopEnd::TokenBudgetReached { .. }
            | LoopEnd::PlanExitRepairExhausted(_)
            | LoopEnd::StopHookLimitReached { .. }
            | LoopEnd::AwaitingApproval => {
                bail!("host execution received an unsupported finalization: {end:?}")
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ToolBatchEffects for HostTurn {
    async fn prepare_call(&mut self, call: &ToolCall) -> Result<ToolAdmission> {
        LlmTurnMetadata::execute()
            .with_cancellation(self.context.cancellation.clone())
            .ensure_not_cancelled()?;
        self.context.inference_agent.context().record_tool_request();
        if self.state.tools.get_tool(&call.name).is_none() {
            self.context
                .inference_agent
                .context()
                .record_tool_rejection();
            let reply = ToolReply::error(format!("Error: unknown tool '{}'", call.name));
            self.record_reply(call, &reply);
            return Ok(ToolAdmission::Reply(reply));
        }
        Ok(ToolAdmission::Invoke)
    }

    async fn invoke_call(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        let tool = self.state.tools.get_tool(&call.name).ok_or_else(|| {
            ToolError::ExecutionFailed(format!("tool '{}' is unavailable", call.name))
        })?;
        let context = ToolCallContext::default()
            .with_session_id(self.events.session_id.as_str())
            .with_turn_id(self.context.turn_id.as_str())
            .with_workspace_root(self.state.workspace_root.clone())
            .with_cancellation(self.context.cancellation.clone())
            .with_inference(self.context.inference_agent.context());
        execute_tool_call(tool, call, context, &mut |progress| {
            self.events.publish(
                Some(&self.context.turn_id),
                RuntimeEvent::Tool(ToolEvent::Progress {
                    call_id: progress.call_id,
                    name: progress.name,
                    event: progress.event,
                }),
            );
        })
        .await
    }

    async fn complete_call(
        &mut self,
        call: &ToolCall,
        result: Result<Value, ToolError>,
    ) -> Result<ToolReply> {
        let reply = match result {
            Ok(value) => ToolReply::success(value.to_string()),
            Err(error) => ToolReply::error(format!("Error: {error}")),
        };
        self.record_reply(call, &reply);
        Ok(reply)
    }
}
