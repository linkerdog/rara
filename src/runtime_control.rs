pub use rara_app_server::runtime_control::{
    ApprovalControlRequest, HookControlRequest, HookLifecycle, InputControlRequest,
    McpControlRequest, MemoryControlRequest, MemoryRecordControlPatch, MemoryScope,
    OutputSubscriptionRequest, PlanApprovalDecision, PromptSourceControlRequest,
    PromptSourceLifetime, PromptSourceRegistration, RuntimeControlEnvelope, RuntimeControlRequest,
    RuntimeControllerKind, RuntimeProvenance, RuntimeSourceAuthorship, RuntimeSourceTrust,
    SessionControlRequest, ShellApprovalDecision, SkillSourceControlRequest, SourceLayer,
    SourceScope,
};
use rara_tools::tool::ToolOutputStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{AgentEvent, BashApprovalDecision, PlanStep, PlanStepStatus};
use crate::context::{ContextObservabilityView, RetrievalOrchestrationView};
use crate::mcp_status::{McpConnectionState, McpStatusSnapshot};
use crate::session_promotion::SessionShardPromotionOutcome;
use crate::todo::TodoState;

impl From<BashApprovalDecision> for ShellApprovalDecision {
    fn from(decision: BashApprovalDecision) -> Self {
        match decision {
            BashApprovalDecision::Once => Self::Once,
            BashApprovalDecision::Prefix => Self::Prefix,
            BashApprovalDecision::Always => Self::Always,
            BashApprovalDecision::Suggestion => Self::Suggestion,
        }
    }
}

impl From<ShellApprovalDecision> for BashApprovalDecision {
    fn from(decision: ShellApprovalDecision) -> Self {
        match decision {
            ShellApprovalDecision::Once => Self::Once,
            ShellApprovalDecision::Prefix => Self::Prefix,
            ShellApprovalDecision::Always => Self::Always,
            ShellApprovalDecision::Suggestion => Self::Suggestion,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeControlEvent {
    pub event_id: String,
    pub provenance: RuntimeProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub sequence: u64,
    pub event: RuntimeEvent,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[allow(clippy::large_enum_variant)]
// RuntimeEvent is the serialized control-plane protocol; boxing one variant
// would complicate consumers and wire compatibility for little gain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RuntimeEvent {
    Session(SessionEvent),
    Input(InputEvent),
    Assistant(AssistantEvent),
    Tool(ToolEvent),
    Approval(ApprovalEvent),
    Plan(PlanEvent),
    PromptSource(PromptSourceEvent),
    Skill(SkillEvent),
    Mcp(McpEvent),
    Memory(MemoryEvent),
    Hook(HookEvent),
    Context(ContextEvent),
    Extension(ExtensionEvent),
    Todo(TodoEvent),
    Warning(WarningEvent),
    Error(ErrorEvent),
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[allow(clippy::enum_variant_names)]
// The suffix keeps context lifecycle variants self-describing in serialized
// RuntimeEvent streams.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SessionEvent {
    Created {
        session_id: String,
    },
    Resumed {
        session_id: String,
    },
    Status {
        message: String,
    },
    TurnStarted,
    TurnCancelled,
    TurnInterrupted,
    TurnFinished {
        reason: Option<String>,
    },
    TurnFailed {
        reason: String,
    },
    ModelRequest {
        model: String,
        /// Estimated input tokens for the outgoing request. A value of 0 means
        /// the provider cannot report the count until the matching response.
        input_tokens: u32,
    },
    ModelResponse {
        model: String,
        output_tokens: u32,
        finish_reason: Option<String>,
    },
    Compacted {
        count: usize,
        before_tokens: usize,
        after_tokens: usize,
        summary: String,
        recent_files: Vec<String>,
    },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[allow(clippy::enum_variant_names)]
// The suffix keeps context lifecycle variants self-describing in serialized
// RuntimeEvent streams.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InputEvent {
    UserPromptSubmitted,
    FollowUpQueued { queue_len: u32 },
    PendingInputAnswered,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[allow(clippy::enum_variant_names)]
// The suffix keeps context lifecycle variants self-describing in serialized
// RuntimeEvent streams.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum AssistantEvent {
    Text(String),
    TextDelta(String),
    ThinkingDelta(String),
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStream {
    Stdout,
    Stderr,
}

impl From<ToolOutputStream> for ToolStream {
    fn from(stream: ToolOutputStream) -> Self {
        match stream {
            ToolOutputStream::Stdout => Self::Stdout,
            ToolOutputStream::Stderr => Self::Stderr,
        }
    }
}

impl From<ToolStream> for ToolOutputStream {
    fn from(stream: ToolStream) -> Self {
        match stream {
            ToolStream::Stdout => Self::Stdout,
            ToolStream::Stderr => Self::Stderr,
        }
    }
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ToolEvent {
    Use {
        #[serde(default)]
        call_id: Option<String>,
        name: String,
        input: Value,
    },
    Result {
        #[serde(default)]
        call_id: Option<String>,
        name: String,
        content: String,
        is_error: bool,
    },
    Progress {
        #[serde(default)]
        call_id: Option<String>,
        name: String,
        stream: ToolStream,
        chunk: String,
    },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ApprovalEvent {
    Requested { approval_id: String, kind: String },
    Answered { approval_id: String, approved: bool },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PlanEvent {
    Updated {
        steps: Vec<PlanStepEvent>,
        explanation: Option<String>,
    },
    Approved,
    Continued,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStepEvent {
    pub step: String,
    pub status: PlanStepStatusEvent,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatusEvent {
    Pending,
    InProgress,
    Completed,
}

impl From<PlanStep> for PlanStepEvent {
    fn from(step: PlanStep) -> Self {
        Self {
            step: step.step,
            status: match step.status {
                PlanStepStatus::Pending => PlanStepStatusEvent::Pending,
                PlanStepStatus::InProgress => PlanStepStatusEvent::InProgress,
                PlanStepStatus::Completed => PlanStepStatusEvent::Completed,
            },
        }
    }
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PromptSourceEvent {
    Registered { source_id: String },
    Injected { source_id: String },
    Unregistered { source_id: String },
    Dropped { source_id: String, reason: String },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SkillEvent {
    Registered { source_id: String, name: String },
    Unregistered { source_id: String, name: String },
    Injected { source_id: String, name: String },
    Shadowed { name: String, by_source_id: String },
    Failed { source_id: String, reason: String },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum McpEvent {
    StatusUpdated {
        snapshot: McpStatusSnapshot,
    },
    StatusLoadFailed {
        message: String,
    },
    ServerStateChanged {
        server_name: String,
        state: McpConnectionState,
    },
    ServerReconnecting {
        server_name: String,
        attempt: u32,
        backoff_ms: u64,
    },
    ConfigurationRefreshed,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum MemoryEvent {
    RecordAdded {
        memory_id: String,
    },
    RecordUpdated {
        memory_id: String,
    },
    RecordDeleted {
        memory_id: String,
    },
    LabelsListed {
        scope: Option<MemoryScope>,
        labels: Vec<MemoryLabelSummary>,
    },
    MetadataQueried {
        record_count: usize,
        labels: Vec<MemoryLabelSummary>,
    },
    RecordsQueried {
        query: String,
        records: Vec<MemoryRecordSummary>,
    },
    ActionObserved {
        message: String,
    },
    SessionShardPromotionObserved {
        outcome: SessionShardPromotionOutcome,
    },
    SelectionUpdated,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryLabelSummary {
    pub label: String,
    pub count: usize,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecordSummary {
    pub id: String,
    pub title: String,
    pub content: String,
    pub labels: Vec<String>,
    pub importance_basis_points: u32,
    pub pinned: bool,
    pub scope: String,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum HookEvent {
    Declared {
        hook_id: String,
        lifecycle: HookLifecycle,
    },
    Injected {
        hook_id: String,
    },
    Ignored {
        hook_id: String,
        reason: String,
    },
    CommandOutput {
        plugin_name: String,
        hook_event: String,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        timed_out: bool,
        ok: bool,
    },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[allow(clippy::enum_variant_names)]
// The suffix keeps context lifecycle variants self-describing in serialized
// RuntimeEvent streams.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ContextEvent {
    SnapshotUpdated,
    RetrievalOrchestrationUpdated { view: RetrievalOrchestrationView },
    ObservabilityUpdated { view: ContextObservabilityView },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ExtensionEvent {
    ReadinessUpdated {
        snapshot: ExtensionReadinessSnapshot,
    },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionReadinessSnapshot {
    pub plugin_count: usize,
    pub hook_count: usize,
    pub skill_count: usize,
    pub command_count: usize,
    pub agent_count: usize,
    pub mcp_server_count: usize,
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TodoEvent {
    Updated { state: TodoState },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WarningEvent {
    RuntimeWarning { message: String },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ErrorEvent {
    RuntimeError { message: String, recoverable: bool },
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
pub fn agent_event_to_runtime_event(event: AgentEvent) -> RuntimeEvent {
    match event {
        AgentEvent::Status(message) => RuntimeEvent::Session(SessionEvent::Status { message }),
        AgentEvent::AssistantText(text) => RuntimeEvent::Assistant(AssistantEvent::Text(text)),
        AgentEvent::AssistantDelta(delta) => {
            RuntimeEvent::Assistant(AssistantEvent::TextDelta(delta))
        }
        AgentEvent::AssistantThinkingDelta(delta) => {
            RuntimeEvent::Assistant(AssistantEvent::ThinkingDelta(delta))
        }
        AgentEvent::ToolUse {
            call_id,
            name,
            input,
        } => RuntimeEvent::Tool(ToolEvent::Use {
            call_id: Some(call_id),
            name,
            input,
        }),
        AgentEvent::ToolResult {
            call_id,
            name,
            content,
            is_error,
        } => RuntimeEvent::Tool(ToolEvent::Result {
            call_id: Some(call_id),
            name,
            content,
            is_error,
        }),
        AgentEvent::ToolProgress {
            call_id,
            name,
            stream,
            chunk,
        } => RuntimeEvent::Tool(ToolEvent::Progress {
            call_id: Some(call_id),
            name,
            stream: stream.into(),
            chunk,
        }),
        AgentEvent::MemoryAction { message } => {
            RuntimeEvent::Memory(MemoryEvent::ActionObserved { message })
        }
        AgentEvent::McpStatusUpdated(snapshot) => {
            RuntimeEvent::Mcp(McpEvent::StatusUpdated { snapshot })
        }
        AgentEvent::McpStatusLoadFailed { message } => {
            RuntimeEvent::Mcp(McpEvent::StatusLoadFailed { message })
        }
        AgentEvent::TodoUpdated(state) => RuntimeEvent::Todo(TodoEvent::Updated { state }),
        AgentEvent::PlanUpdated { steps, explanation } => RuntimeEvent::Plan(PlanEvent::Updated {
            steps: steps.into_iter().map(PlanStepEvent::from).collect(),
            explanation,
        }),
        AgentEvent::ApprovalRequested { approval_id, kind } => {
            RuntimeEvent::Approval(ApprovalEvent::Requested { approval_id, kind })
        }
        AgentEvent::ApprovalAnswered {
            approval_id,
            approved,
        } => RuntimeEvent::Approval(ApprovalEvent::Answered {
            approval_id,
            approved,
        }),
        AgentEvent::AgentStart => RuntimeEvent::Session(SessionEvent::TurnStarted),
        AgentEvent::AgentStop { reason } => RuntimeEvent::Session(SessionEvent::TurnFinished {
            reason: Some(reason),
        }),
        AgentEvent::AgentError {
            message,
            recoverable,
        } => RuntimeEvent::Error(ErrorEvent::RuntimeError {
            message,
            recoverable,
        }),
        AgentEvent::ModelRequest {
            model,
            input_tokens,
        } => RuntimeEvent::Session(SessionEvent::ModelRequest {
            model,
            input_tokens,
        }),
        AgentEvent::ModelResponse {
            model,
            output_tokens,
            finish_reason,
        } => RuntimeEvent::Session(SessionEvent::ModelResponse {
            model,
            output_tokens,
            finish_reason,
        }),
        AgentEvent::Compaction {
            count,
            before_tokens,
            after_tokens,
            summary,
            recent_files,
        } => RuntimeEvent::Session(SessionEvent::Compacted {
            count,
            before_tokens,
            after_tokens,
            summary,
            recent_files,
        }),
    }
}

#[allow(dead_code)] // ACP protocol type — reserved for future lifecycle events
pub fn wrap_agent_event(
    event_id: impl Into<String>,
    sequence: u64,
    provenance: RuntimeProvenance,
    event: AgentEvent,
) -> RuntimeControlEvent {
    RuntimeControlEvent {
        event_id: event_id.into(),
        provenance,
        turn_id: None,
        sequence,
        event: agent_event_to_runtime_event(event),
    }
}

include!("runtime_control/tests.rs");
