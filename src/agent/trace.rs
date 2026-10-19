use rara_agent_trace::{
    AgentStepUpdated, AgentTraceEvent, AgentTraceRecorder, CacheUsage, ContextAssembled,
    ModelFinished, ModelUsage, TraceModelStatus, TraceTurnOutcome, TurnFinished, TurnStarted,
};

use super::Agent;
use crate::context::{SharedRuntimeContext, is_retrieved_memory_kind};
use crate::llm::TokenUsage;

impl Agent {
    pub(crate) fn set_agent_trace_recorder(&mut self, recorder: AgentTraceRecorder) {
        self.agent_trace = recorder;
    }

    pub(super) fn record_agent_trace_turn_started(&self) {
        if !self.agent_trace.is_enabled() {
            return;
        }
        self.record_agent_trace_event(AgentTraceEvent::TurnStarted(TurnStarted {
            history_len: self.history.len(),
            memory_facilities_enabled: self.memory_facilities_enabled,
        }));
    }

    pub(super) fn record_agent_trace_turn_finished(&self, succeeded: bool) {
        if !self.agent_trace.is_enabled() {
            return;
        }
        let outcome = if succeeded {
            TraceTurnOutcome::Succeeded
        } else {
            TraceTurnOutcome::Failed
        };
        self.record_agent_trace_event(AgentTraceEvent::TurnFinished(TurnFinished {
            outcome,
            model_turn_count: self.last_query_report.model_turns.len(),
        }));
    }

    pub(super) fn record_agent_trace_context_assembled(&self, context: &SharedRuntimeContext) {
        if !self.agent_trace.is_enabled() {
            return;
        }
        let selection = &context.retrieval.memory_selection;
        let (selected_memory_count, selected_memory_tokens) = selection
            .selected_items
            .iter()
            .filter(|item| is_retrieved_memory_kind(item.kind.as_str()))
            .fold((0_usize, 0_usize), |(count, tokens), item| {
                (
                    count.saturating_add(1),
                    tokens.saturating_add(item.budget_impact_tokens.unwrap_or_default()),
                )
            });
        let retrieval = &context.retrieval.orchestration;
        self.record_agent_trace_event(AgentTraceEvent::ContextAssembled(ContextAssembled {
            candidate_count: retrieval.candidates.len(),
            selected_count: retrieval.selected.len(),
            available_count: retrieval.available.len(),
            dropped_count: retrieval.dropped.len(),
            selected_tokens: retrieval.budget.selected_tokens,
            available_tokens: retrieval.budget.available_tokens,
            dropped_tokens: retrieval.budget.dropped_tokens,
            selected_memory_count,
            selected_memory_tokens,
        }));
    }

    pub(super) fn record_agent_trace_model_finished(
        &self,
        model: String,
        duration_ms: u64,
        status: TraceModelStatus,
        finish_reason: Option<String>,
        usage: Option<&TokenUsage>,
    ) {
        if !self.agent_trace.is_enabled() {
            return;
        }
        let usage = usage.map(|usage| ModelUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache: (usage.cache_hit_tokens != 0 || usage.cache_miss_tokens != 0).then_some(
                CacheUsage {
                    hit_tokens: usage.cache_hit_tokens,
                    miss_tokens: usage.cache_miss_tokens,
                },
            ),
        });
        self.record_agent_trace_event(AgentTraceEvent::ModelFinished(ModelFinished {
            model,
            duration_ms,
            status,
            finish_reason,
            usage,
        }));
    }

    pub(super) fn record_agent_trace_step(&self) {
        if !self.agent_trace.is_enabled() {
            return;
        }
        let step = &self.last_agent_turn_trace;
        self.record_agent_trace_event(AgentTraceEvent::AgentStepUpdated(AgentStepUpdated {
            agentic_turn_index: step.agentic_turn_index,
            execution_mode: step.execution_mode.clone(),
            model_stop_reason: step.model_stop_reason.clone(),
            loop_outcome: step.loop_outcome.clone(),
            continuation_phase: step.continuation_phase.clone(),
            had_text_response: step.had_text_response,
            had_reasoning_response: step.had_reasoning_response,
            reasoning_only: step.reasoning_only,
            streamed_text_delta: step.streamed_text_delta,
            streamed_reasoning_delta: step.streamed_reasoning_delta,
            assistant_message_recorded: step.assistant_message_recorded,
            tool_call_count: step.tool_call_count,
            plan_updated: step.plan_updated,
            continue_inspection: step.continue_inspection,
            malformed_proposed_plan: step.malformed_proposed_plan,
        }));
    }

    fn record_agent_trace_event(&self, event: AgentTraceEvent) {
        if let Err(error) = self
            .agent_trace
            .record(self.runtime_turn_id.as_deref(), event)
        {
            log::warn!("Failed to write agent trace event: {error}");
        }
    }
}
