use rara_tools::tool::ToolOutputStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{AgentEvent, BashApprovalDecision};
use crate::context::{ContextObservabilityView, RetrievalOrchestrationView};
use crate::mcp_status::{McpConnectionState, McpStatusSnapshot};
use crate::session_promotion::SessionShardPromotionOutcome;
use crate::todo::TodoState;

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeControllerKind {
    LocalTui,
    LocalCli,
    Acp,
    Wire,
    AppServer,
    Runtime,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProvenance {
    pub controller: RuntimeControllerKind,
    pub adapter: Option<String>,
    pub session_id: Option<String>,
    pub source_id: Option<String>,
    pub trust: RuntimeSourceTrust,
    pub authorship: RuntimeSourceAuthorship,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceTrust {
    Trusted,
    Untrusted,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceAuthorship {
    UserProvided,
    Generated,
    Runtime,
}

#[allow(dead_code)]
impl RuntimeProvenance {
    pub fn local_tui(session_id: impl Into<String>) -> Self {
        Self {
            controller: RuntimeControllerKind::LocalTui,
            adapter: None,
            session_id: Some(session_id.into()),
            source_id: None,
            trust: RuntimeSourceTrust::Trusted,
            authorship: RuntimeSourceAuthorship::UserProvided,
        }
    }

    pub fn runtime(session_id: Option<String>) -> Self {
        Self {
            controller: RuntimeControllerKind::Runtime,
            adapter: None,
            session_id,
            source_id: None,
            trust: RuntimeSourceTrust::Trusted,
            authorship: RuntimeSourceAuthorship::Runtime,
        }
    }

    pub fn protocol(
        controller: RuntimeControllerKind,
        adapter: impl Into<String>,
        session_id: Option<String>,
        source_id: Option<String>,
    ) -> Self {
        Self {
            controller,
            adapter: Some(adapter.into()),
            session_id,
            source_id,
            trust: RuntimeSourceTrust::Untrusted,
            authorship: RuntimeSourceAuthorship::UserProvided,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RuntimeControlRequest {
    Session(SessionControlRequest),
    Input(InputControlRequest),
    Output(OutputSubscriptionRequest),
    PromptSource(PromptSourceControlRequest),
    SkillSource(SkillSourceControlRequest),
    Mcp(McpControlRequest),
    Memory(MemoryControlRequest),
    Hook(HookControlRequest),
    Approval(ApprovalControlRequest),
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeControlEnvelope {
    pub request_id: String,
    pub provenance: RuntimeProvenance,
    pub request: RuntimeControlRequest,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SessionControlRequest {
    CreateSession,
    ResumeSession { session_id: String },
    CancelCurrentTurn,
    InterruptCurrentTurn,
    QueryRuntimeState,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InputControlRequest {
    SubmitUserPrompt { prompt: String },
    AnswerPendingInput { answer: String },
    AnswerPlanApproval { approved: bool },
    AnswerShellApproval { decision: ShellApprovalDecision },
    SubmitFollowUp { prompt: String },
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellApprovalDecision {
    Once,
    Prefix,
    Always,
    Suggestion,
}

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

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum OutputSubscriptionRequest {
    Subscribe { subscriber_id: String },
    Unsubscribe { subscriber_id: String },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PromptSourceLifetime {
    Turns(u32),
    Session,
    Persistent,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSourceRegistration {
    pub source_id: String,
    pub scope: SourceScope,
    pub layer: SourceLayer,
    pub budget_hint_tokens: Option<u32>,
    pub lifetime: PromptSourceLifetime,
    pub content: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    Home,
    Repo,
    CurrentWorkingDirectory,
    Session,
    Protocol,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLayer {
    System,
    Developer,
    User,
    Memory,
    Skill,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PromptSourceControlRequest {
    Register(PromptSourceRegistration),
    Unregister { source_id: String },
    QuerySources,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SkillSourceControlRequest {
    RegisterRoot {
        source_id: String,
        root: String,
        precedence_hint: Option<i32>,
    },
    RegisterSkill {
        source_id: String,
        name: String,
        content: String,
        precedence_hint: Option<i32>,
    },
    DisableSkill {
        name: String,
        source_id: Option<String>,
    },
    QuerySkills,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum McpControlRequest {
    QueryStatus,
    Refresh { server_name: Option<String> },
    Reconnect { server_name: String },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum MemoryControlRequest {
    AddRecord {
        memory_id: String,
        scope: MemoryScope,
        content: String,
        metadata: Value,
    },
    UpdateRecord {
        memory_id: String,
        patch: MemoryRecordControlPatch,
    },
    DeleteRecord {
        memory_id: String,
    },
    ListLabels {
        scope: Option<MemoryScope>,
    },
    QueryRecords {
        query: String,
        scope: Option<MemoryScope>,
        limit: usize,
    },
    QueryMetadata,
    SelectionSnapshot,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecordControlPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MemoryScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Option<String>>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Thread,
    Workspace,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum HookControlRequest {
    Declare {
        hook_id: String,
        lifecycle: HookLifecycle,
        description: String,
    },
    QueryHooks,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookLifecycle {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    PreCompact,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ApprovalControlRequest {
    AnswerPendingApproval { approval_id: String, approved: bool },
    QueryPendingApprovals,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeControlEvent {
    pub event_id: String,
    pub provenance: RuntimeProvenance,
    pub sequence: u64,
    pub event: RuntimeEvent,
}

#[allow(dead_code)]
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
    Todo(TodoEvent),
    Warning(WarningEvent),
    Error(ErrorEvent),
}

#[allow(dead_code)]
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
    ModelRequest {
        model: String,
        input_tokens: u32,
    },
    ModelResponse {
        model: String,
        output_tokens: u32,
        finish_reason: Option<String>,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InputEvent {
    UserPromptSubmitted,
    FollowUpQueued { queue_len: u32 },
    PendingInputAnswered,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum AssistantEvent {
    Text(String),
    TextDelta(String),
    ThinkingDelta(String),
}

#[allow(dead_code)]
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

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ToolEvent {
    Use {
        name: String,
        input: Value,
    },
    Result {
        name: String,
        content: String,
        is_error: bool,
    },
    Progress {
        name: String,
        stream: ToolStream,
        chunk: String,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ApprovalEvent {
    Requested { approval_id: String, kind: String },
    Answered { approval_id: String, approved: bool },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PlanEvent {
    Updated,
    Approved,
    Continued,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PromptSourceEvent {
    Registered { source_id: String },
    Injected { source_id: String },
    Unregistered { source_id: String },
    Dropped { source_id: String, reason: String },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SkillEvent {
    Registered { source_id: String, name: String },
    Unregistered { source_id: String, name: String },
    Injected { source_id: String, name: String },
    Shadowed { name: String, by_source_id: String },
    Failed { source_id: String, reason: String },
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
        records: Vec<MemoryRecordSummary>,
    },
    SessionShardPromotionObserved {
        outcome: SessionShardPromotionOutcome,
    },
    SelectionUpdated,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryLabelSummary {
    pub label: String,
    pub count: usize,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ContextEvent {
    SnapshotUpdated,
    RetrievalOrchestrationUpdated { view: RetrievalOrchestrationView },
    ObservabilityUpdated { view: ContextObservabilityView },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TodoEvent {
    Updated { state: TodoState },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WarningEvent {
    RuntimeWarning { message: String },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ErrorEvent {
    RuntimeError { message: String, recoverable: bool },
}

#[allow(dead_code)]
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
        AgentEvent::ToolUse { name, input } => RuntimeEvent::Tool(ToolEvent::Use { name, input }),
        AgentEvent::ToolResult {
            name,
            content,
            is_error,
        } => RuntimeEvent::Tool(ToolEvent::Result {
            name,
            content,
            is_error,
        }),
        AgentEvent::ToolProgress {
            name,
            stream,
            chunk,
        } => RuntimeEvent::Tool(ToolEvent::Progress {
            name,
            stream: stream.into(),
            chunk,
        }),
        AgentEvent::McpStatusUpdated(snapshot) => {
            RuntimeEvent::Mcp(McpEvent::StatusUpdated { snapshot })
        }
        AgentEvent::McpStatusLoadFailed { message } => {
            RuntimeEvent::Mcp(McpEvent::StatusLoadFailed { message })
        }
        AgentEvent::TodoUpdated(state) => RuntimeEvent::Todo(TodoEvent::Updated { state }),
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
    }
}

#[allow(dead_code)]
pub fn wrap_agent_event(
    event_id: impl Into<String>,
    sequence: u64,
    provenance: RuntimeProvenance,
    event: AgentEvent,
) -> RuntimeControlEvent {
    RuntimeControlEvent {
        event_id: event_id.into(),
        provenance,
        sequence,
        event: agent_event_to_runtime_event(event),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn agent_tool_progress_maps_to_structured_runtime_event() {
        let event = agent_event_to_runtime_event(AgentEvent::ToolProgress {
            name: "bash".to_string(),
            stream: ToolOutputStream::Stderr,
            chunk: "error\n".to_string(),
        });

        assert_eq!(
            event,
            RuntimeEvent::Tool(ToolEvent::Progress {
                name: "bash".to_string(),
                stream: ToolStream::Stderr,
                chunk: "error\n".to_string(),
            })
        );
    }

    #[test]
    fn agent_status_maps_to_session_status_not_warning() {
        let event = agent_event_to_runtime_event(AgentEvent::Status("Sending prompt.".to_string()));

        assert_eq!(
            event,
            RuntimeEvent::Session(SessionEvent::Status {
                message: "Sending prompt.".to_string()
            })
        );
    }

    #[test]
    fn prompt_source_lifetime_serializes_as_turn_based_contract() {
        let request = RuntimeControlEnvelope {
            request_id: "req-1".to_string(),
            provenance: RuntimeProvenance::protocol(
                RuntimeControllerKind::Acp,
                "acp",
                Some("session-1".to_string()),
                Some("source-1".to_string()),
            ),
            request: RuntimeControlRequest::PromptSource(PromptSourceControlRequest::Register(
                PromptSourceRegistration {
                    source_id: "source-1".to_string(),
                    scope: SourceScope::Protocol,
                    layer: SourceLayer::User,
                    budget_hint_tokens: Some(256),
                    lifetime: PromptSourceLifetime::Turns(2),
                    content: "adapter context".to_string(),
                },
            )),
        };

        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(value["request_id"], json!("req-1"));
        assert_eq!(value["provenance"]["controller"], json!("acp"));
        assert_eq!(value["provenance"]["trust"], json!("untrusted"));
        assert_eq!(value["provenance"]["authorship"], json!("user_provided"));
        assert_eq!(
            value["request"],
            json!({
                "type": "prompt_source",
                "payload": {
                    "type": "register",
                    "payload": {
                        "source_id": "source-1",
                        "scope": "protocol",
                        "layer": "user",
                        "budget_hint_tokens": 256,
                        "lifetime": {
                            "type": "turns",
                            "payload": 2
                        },
                        "content": "adapter context"
                    }
                }
            })
        );
    }

    #[test]
    fn shell_approval_decision_round_trips_runtime_decision() {
        for (runtime, contract) in [
            (BashApprovalDecision::Once, ShellApprovalDecision::Once),
            (BashApprovalDecision::Prefix, ShellApprovalDecision::Prefix),
            (BashApprovalDecision::Always, ShellApprovalDecision::Always),
            (
                BashApprovalDecision::Suggestion,
                ShellApprovalDecision::Suggestion,
            ),
        ] {
            assert_eq!(ShellApprovalDecision::from(runtime), contract);
            assert_eq!(BashApprovalDecision::from(contract), runtime);
        }
    }

    #[test]
    fn input_event_uses_fixed_width_queue_length_and_stable_wire_shape() {
        let value = serde_json::to_value(RuntimeEvent::Input(InputEvent::FollowUpQueued {
            queue_len: 3,
        }))
        .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "input",
                "payload": {
                    "type": "follow_up_queued",
                    "payload": {
                        "queue_len": 3
                    }
                }
            })
        );
    }

    #[test]
    fn todo_updated_event_uses_structured_wire_shape() {
        let state = crate::todo::normalize_todo_write_input(&json!({
            "todos": [
                {"content": "Implement todo runtime", "status": "in_progress"}
            ]
        }))
        .expect("todo state");
        let value =
            serde_json::to_value(agent_event_to_runtime_event(AgentEvent::TodoUpdated(state)))
                .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "todo",
                "payload": {
                    "type": "updated",
                    "payload": {
                        "state": {
                            "version": 1,
                            "items": [
                                {
                                    "id": "todo-1",
                                    "content": "Implement todo runtime",
                                    "status": "in_progress",
                                    "updated_at": value["payload"]["payload"]["state"]["updated_at"]
                                }
                            ],
                            "updated_at": value["payload"]["payload"]["state"]["updated_at"]
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn mcp_status_update_event_uses_structured_wire_shape() {
        let value = serde_json::to_value(agent_event_to_runtime_event(
            AgentEvent::McpStatusUpdated(McpStatusSnapshot { servers: vec![] }),
        ))
        .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "mcp",
                "payload": {
                    "type": "status_updated",
                    "payload": {
                        "snapshot": {
                            "servers": []
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn mcp_status_load_failed_event_uses_structured_wire_shape() {
        let value = serde_json::to_value(agent_event_to_runtime_event(
            AgentEvent::McpStatusLoadFailed {
                message: "invalid config".to_string(),
            },
        ))
        .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "mcp",
                "payload": {
                    "type": "status_load_failed",
                    "payload": {
                        "message": "invalid config"
                    }
                }
            })
        );
    }

    #[test]
    fn mcp_control_requests_use_structured_wire_shape() {
        let query = RuntimeControlRequest::Mcp(McpControlRequest::QueryStatus);
        assert_eq!(
            serde_json::to_value(query).unwrap(),
            json!({
                "type": "mcp",
                "payload": {
                    "type": "query_status"
                }
            })
        );

        let refresh = RuntimeControlRequest::Mcp(McpControlRequest::Refresh {
            server_name: Some("docs".to_string()),
        });
        assert_eq!(
            serde_json::to_value(refresh).unwrap(),
            json!({
                "type": "mcp",
                "payload": {
                    "type": "refresh",
                    "payload": {
                        "server_name": "docs"
                    }
                }
            })
        );

        let reconnect = RuntimeControlRequest::Mcp(McpControlRequest::Reconnect {
            server_name: "docs".to_string(),
        });
        assert_eq!(
            serde_json::to_value(reconnect).unwrap(),
            json!({
                "type": "mcp",
                "payload": {
                    "type": "reconnect",
                    "payload": {
                        "server_name": "docs"
                    }
                }
            })
        );
    }

    #[test]
    fn retrieval_orchestration_event_uses_structured_wire_shape() {
        let event = RuntimeEvent::Context(ContextEvent::RetrievalOrchestrationUpdated {
            view: RetrievalOrchestrationView {
                request_id: "session-1".to_string(),
                query: "where is the reference project?".to_string(),
                providers: vec![crate::context::RetrievalProviderStatus {
                    order: 1,
                    kind: "vector_memory".to_string(),
                    label: "Vector Memory Store".to_string(),
                    status: "available".to_string(),
                    detail: "memory://vdb".to_string(),
                    inclusion_reason: "configured as durable memory".to_string(),
                }],
                candidates: vec![crate::context::RetrievalCandidateContextEntry {
                    order: 1,
                    kind: "retrieved_workspace_memory".to_string(),
                    label: "Memory: reference project".to_string(),
                    detail: "content: reference project path".to_string(),
                    status: "selected".to_string(),
                    source_kind: "memory_record".to_string(),
                    budget_impact_tokens: Some(11),
                    reason: "selected for current turn".to_string(),
                }],
                selected: vec![crate::context::RetrievalCandidateContextEntry {
                    order: 1,
                    kind: "retrieved_workspace_memory".to_string(),
                    label: "Memory: reference project".to_string(),
                    detail: "content: reference project path".to_string(),
                    status: "selected".to_string(),
                    source_kind: "memory_record".to_string(),
                    budget_impact_tokens: Some(11),
                    reason: "selected for current turn".to_string(),
                }],
                available: Vec::new(),
                dropped: Vec::new(),
                budget: crate::context::RetrievalBudgetContextView {
                    selection_budget_tokens: Some(100),
                    selected_tokens: 11,
                    available_tokens: 0,
                    dropped_tokens: 0,
                },
            },
        });

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "type": "context",
                "payload": {
                    "type": "retrieval_orchestration_updated",
                    "payload": {
                        "view": {
                            "request_id": "session-1",
                            "query": "where is the reference project?",
                            "providers": [{
                                "order": 1,
                                "kind": "vector_memory",
                                "label": "Vector Memory Store",
                                "status": "available",
                                "detail": "memory://vdb",
                                "inclusion_reason": "configured as durable memory"
                            }],
                            "candidates": [{
                                "order": 1,
                                "kind": "retrieved_workspace_memory",
                                "label": "Memory: reference project",
                                "detail": "content: reference project path",
                                "status": "selected",
                                "source_kind": "memory_record",
                                "budget_impact_tokens": 11,
                                "reason": "selected for current turn"
                            }],
                            "selected": [{
                                "order": 1,
                                "kind": "retrieved_workspace_memory",
                                "label": "Memory: reference project",
                                "detail": "content: reference project path",
                                "status": "selected",
                                "source_kind": "memory_record",
                                "budget_impact_tokens": 11,
                                "reason": "selected for current turn"
                            }],
                            "available": [],
                            "dropped": [],
                            "budget": {
                                "selection_budget_tokens": 100,
                                "selected_tokens": 11,
                                "available_tokens": 0,
                                "dropped_tokens": 0
                            }
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn context_observability_event_uses_structured_wire_shape() {
        let event = RuntimeEvent::Context(ContextEvent::ObservabilityUpdated {
            view: ContextObservabilityView {
                cache: crate::context::ContextCacheObservationView {
                    hit_tokens: 90,
                    miss_tokens: 10,
                    hit_rate_basis_points: Some(9000),
                },
                compaction: crate::context::ContextCompactionObservationView {
                    estimated_history_tokens: 12_000,
                    compact_threshold_tokens: 10_000,
                    compaction_count: 2,
                    last_before_tokens: Some(9_000),
                    last_after_tokens: Some(3_000),
                    last_saved_tokens: Some(6_000),
                },
                microcompact: crate::context::MicrocompactProjectionContextView {
                    enabled: true,
                    budget_chars: 48_000,
                    keep_recent: 6,
                    cache_edit_eligible: false,
                    cache_edit_applied: false,
                    original_chars: 60_000,
                    projected_chars: 30_000,
                    saved_chars: 30_000,
                    cleared_results: 4,
                    kept_results: 6,
                },
                retrieval: crate::context::RetrievalObservationView {
                    request_id: "session-1".to_string(),
                    provider_count: 3,
                    candidate_count: 5,
                    selected_count: 2,
                    available_count: 2,
                    dropped_count: 1,
                    selected_tokens: 500,
                    available_tokens: 300,
                    dropped_tokens: 200,
                },
                agent_turn: crate::context::AgentTurnTraceView {
                    agentic_turn_index: 1,
                    execution_mode: "execute".to_string(),
                    model_stop_reason: Some("end_turn".to_string()),
                    loop_outcome: Some("continued".to_string()),
                    continuation_phase: Some("reasoning_only_continuation_required".to_string()),
                    had_text_response: false,
                    had_reasoning_response: true,
                    reasoning_only: true,
                    streamed_text_delta: false,
                    streamed_reasoning_delta: true,
                    assistant_message_recorded: false,
                    tool_call_count: 0,
                    plan_updated: false,
                    continue_inspection: false,
                    malformed_proposed_plan: false,
                },
            },
        });

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "type": "context",
                "payload": {
                    "type": "observability_updated",
                    "payload": {
                        "view": {
                            "cache": {
                                "hit_tokens": 90,
                                "miss_tokens": 10,
                                "hit_rate_basis_points": 9000
                            },
                            "compaction": {
                                "estimated_history_tokens": 12000,
                                "compact_threshold_tokens": 10000,
                                "compaction_count": 2,
                                "last_before_tokens": 9000,
                                "last_after_tokens": 3000,
                                "last_saved_tokens": 6000
                            },
                            "microcompact": {
                                "enabled": true,
                                "budget_chars": 48000,
                                "keep_recent": 6,
                                "cache_edit_eligible": false,
                                "cache_edit_applied": false,
                                "original_chars": 60000,
                                "projected_chars": 30000,
                                "saved_chars": 30000,
                                "cleared_results": 4,
                                "kept_results": 6
                            },
                            "retrieval": {
                                "request_id": "session-1",
                                "provider_count": 3,
                                "candidate_count": 5,
                                "selected_count": 2,
                                "available_count": 2,
                                "dropped_count": 1,
                                "selected_tokens": 500,
                                "available_tokens": 300,
                                "dropped_tokens": 200
                            },
                            "agent_turn": {
                                "agentic_turn_index": 1,
                                "execution_mode": "execute",
                                "model_stop_reason": "end_turn",
                                "loop_outcome": "continued",
                                "continuation_phase": "reasoning_only_continuation_required",
                                "had_text_response": false,
                                "had_reasoning_response": true,
                                "reasoning_only": true,
                                "streamed_text_delta": false,
                                "streamed_reasoning_delta": true,
                                "assistant_message_recorded": false,
                                "tool_call_count": 0,
                                "plan_updated": false,
                                "continue_inspection": false,
                                "malformed_proposed_plan": false
                            }
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn memory_update_delete_and_label_requests_use_structured_wire_shape() {
        let update = serde_json::to_value(RuntimeControlRequest::Memory(
            MemoryControlRequest::UpdateRecord {
                memory_id: "memory-1".to_string(),
                patch: MemoryRecordControlPatch {
                    title: Some("Updated memory".to_string()),
                    labels: Some(vec!["decision".to_string(), "fact".to_string()]),
                    importance: Some(0.9),
                    pinned: Some(true),
                    scope: Some(MemoryScope::Workspace),
                    thread_id: Some(None),
                    ..Default::default()
                },
            },
        ))
        .unwrap();
        assert_eq!(
            update,
            json!({
                "type": "memory",
                "payload": {
                    "type": "update_record",
                    "payload": {
                        "memory_id": "memory-1",
                        "patch": {
                            "title": "Updated memory",
                            "labels": ["decision", "fact"],
                            "importance": 0.9,
                            "pinned": true,
                            "scope": "workspace",
                            "thread_id": null
                        }
                    }
                }
            })
        );

        let delete = serde_json::to_value(RuntimeControlRequest::Memory(
            MemoryControlRequest::DeleteRecord {
                memory_id: "memory-1".to_string(),
            },
        ))
        .unwrap();
        assert_eq!(
            delete,
            json!({
                "type": "memory",
                "payload": {
                    "type": "delete_record",
                    "payload": {
                        "memory_id": "memory-1"
                    }
                }
            })
        );

        let labels = serde_json::to_value(RuntimeControlRequest::Memory(
            MemoryControlRequest::ListLabels {
                scope: Some(MemoryScope::Thread),
            },
        ))
        .unwrap();
        assert_eq!(
            labels,
            json!({
                "type": "memory",
                "payload": {
                    "type": "list_labels",
                    "payload": {
                        "scope": "thread"
                    }
                }
            })
        );

        let query = serde_json::to_value(RuntimeControlRequest::Memory(
            MemoryControlRequest::QueryRecords {
                query: "project path".to_string(),
                scope: Some(MemoryScope::Workspace),
                limit: 4,
            },
        ))
        .unwrap();
        assert_eq!(
            query,
            json!({
                "type": "memory",
                "payload": {
                    "type": "query_records",
                    "payload": {
                        "query": "project path",
                        "scope": "workspace",
                        "limit": 4
                    }
                }
            })
        );
    }

    #[test]
    fn memory_label_and_metadata_events_use_structured_wire_shape() {
        let labels = serde_json::to_value(RuntimeEvent::Memory(MemoryEvent::LabelsListed {
            scope: Some(MemoryScope::Workspace),
            labels: vec![MemoryLabelSummary {
                label: "decision".to_string(),
                count: 2,
            }],
        }))
        .unwrap();
        assert_eq!(
            labels,
            json!({
                "type": "memory",
                "payload": {
                    "type": "labels_listed",
                    "payload": {
                        "scope": "workspace",
                        "labels": [{"label": "decision", "count": 2}]
                    }
                }
            })
        );

        let metadata = serde_json::to_value(RuntimeEvent::Memory(MemoryEvent::MetadataQueried {
            record_count: 3,
            labels: vec![MemoryLabelSummary {
                label: "fact".to_string(),
                count: 1,
            }],
        }))
        .unwrap();
        assert_eq!(
            metadata,
            json!({
                "type": "memory",
                "payload": {
                    "type": "metadata_queried",
                    "payload": {
                        "record_count": 3,
                        "labels": [{"label": "fact", "count": 1}]
                    }
                }
            })
        );

        let records = serde_json::to_value(RuntimeEvent::Memory(MemoryEvent::RecordsQueried {
            records: vec![MemoryRecordSummary {
                id: "memory-1".to_string(),
                title: "Reference project path".to_string(),
                content: "The local project is under /repo.".to_string(),
                labels: vec!["fact".to_string()],
                importance_basis_points: 7500,
                pinned: true,
                scope: "workspace".to_string(),
                session_id: Some("session-1".to_string()),
                thread_id: None,
            }],
        }))
        .unwrap();
        assert_eq!(
            records,
            json!({
                "type": "memory",
                "payload": {
                    "type": "records_queried",
                    "payload": {
                        "records": [{
                            "id": "memory-1",
                            "title": "Reference project path",
                            "content": "The local project is under /repo.",
                            "labels": ["fact"],
                            "importance_basis_points": 7500,
                            "pinned": true,
                            "scope": "workspace",
                            "session_id": "session-1",
                            "thread_id": null
                        }]
                    }
                }
            })
        );

        let promotion = serde_json::to_value(RuntimeEvent::Memory(
            MemoryEvent::SessionShardPromotionObserved {
                outcome: crate::session_promotion::SessionShardPromotionOutcome {
                    plan: crate::session_promotion::SessionShardPromotionPlan {
                        session_id: "session-1".to_string(),
                        trigger: crate::session_promotion::SessionShardPromotionTrigger::Periodic,
                        checkpoint_count: 2,
                        min_checkpoints: 2,
                        max_checkpoints: 8,
                        decision: crate::session_promotion::SessionShardPromotionDecision::Skipped {
                            reason: crate::session_promotion::SessionShardPromotionSkipReason::Disabled,
                        },
                    },
                    promoted_count: 0,
                },
            },
        ))
        .unwrap();
        assert_eq!(
            promotion,
            json!({
                "type": "memory",
                "payload": {
                    "type": "session_shard_promotion_observed",
                    "payload": {
                        "outcome": {
                            "plan": {
                                "session_id": "session-1",
                                "trigger": "periodic",
                                "checkpoint_count": 2,
                                "min_checkpoints": 2,
                                "max_checkpoints": 8,
                                "decision": {
                                    "status": "skipped",
                                    "reason": "disabled"
                                }
                            },
                            "promoted_count": 0
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn wrapped_agent_event_preserves_provenance_and_sequence() {
        let control_event = wrap_agent_event(
            "evt-1",
            42,
            RuntimeProvenance::local_tui("session-1"),
            AgentEvent::AssistantDelta("hello".to_string()),
        );

        assert_eq!(control_event.event_id, "evt-1");
        assert_eq!(control_event.sequence, 42);
        assert_eq!(
            control_event.provenance.controller,
            RuntimeControllerKind::LocalTui
        );
        assert_eq!(
            control_event.provenance.session_id.as_deref(),
            Some("session-1")
        );
        assert_eq!(
            control_event.event,
            RuntimeEvent::Assistant(AssistantEvent::TextDelta("hello".to_string()))
        );
    }
}
