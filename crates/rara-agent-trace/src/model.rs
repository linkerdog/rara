use serde::{Deserialize, Serialize};

/// Version of the local JSONL schema.
pub const AGENT_TRACE_SCHEMA_VERSION: u16 = 1;

/// Immutable metadata for one local agent trace directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceManifest {
    pub schema_version: u16,
    pub session_id: String,
    pub created_at_unix_ms: u64,
}

/// One ordered, content-free observation from an agent session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRecord {
    pub schema_version: u16,
    pub sequence: u64,
    pub timestamp_unix_ms: u64,
    pub elapsed_ms: u64,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub event: AgentTraceEvent,
}

/// Typed event payload written to the local trace stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentTraceEvent {
    TurnStarted(TurnStarted),
    ContextAssembled(ContextAssembled),
    ModelFinished(ModelFinished),
    AgentStepUpdated(AgentStepUpdated),
    TurnFinished(TurnFinished),
}

/// Metadata captured when one user turn enters the agent loop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStarted {
    pub history_len: usize,
    pub memory_facilities_enabled: bool,
}

/// Content-free context selection for one model request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAssembled {
    pub candidate_count: usize,
    pub selected_count: usize,
    pub available_count: usize,
    pub dropped_count: usize,
    pub selected_tokens: usize,
    pub available_tokens: usize,
    pub dropped_tokens: usize,
    pub selected_memory_count: usize,
    pub selected_memory_tokens: usize,
}

/// Provider-reported prompt-cache accounting for one model request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheUsage {
    pub hit_tokens: u32,
    pub miss_tokens: u32,
}

impl CacheUsage {
    /// Return the hit rate only when the provider reported a non-empty cache receipt.
    pub fn hit_rate_basis_points(self) -> Option<u16> {
        let total = u64::from(self.hit_tokens).saturating_add(u64::from(self.miss_tokens));
        if total == 0 {
            return None;
        }
        let basis_points = u64::from(self.hit_tokens)
            .saturating_mul(10_000)
            .checked_div(total)?;
        Some(basis_points.min(u64::from(u16::MAX)) as u16)
    }
}

/// Token receipt reported by a provider for one completed model request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// `None` means the provider did not return usable prompt-cache accounting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheUsage>,
}

/// Terminal state for one model request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceModelStatus {
    Succeeded,
    Failed,
}

/// One completed model request without its prompt or response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFinished {
    pub model: String,
    pub duration_ms: u64,
    pub status: TraceModelStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsage>,
}

/// Snapshot of one agent-loop transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStepUpdated {
    pub agentic_turn_index: usize,
    pub execution_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_phase: Option<String>,
    pub had_text_response: bool,
    pub had_reasoning_response: bool,
    pub reasoning_only: bool,
    pub streamed_text_delta: bool,
    pub streamed_reasoning_delta: bool,
    pub assistant_message_recorded: bool,
    pub tool_call_count: usize,
    pub plan_updated: bool,
    pub continue_inspection: bool,
    pub malformed_proposed_plan: bool,
}

/// Terminal outcome for one user turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceTurnOutcome {
    Succeeded,
    Failed,
}

/// Metadata emitted after a user turn returns to its caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnFinished {
    pub outcome: TraceTurnOutcome,
    pub model_turn_count: usize,
}
