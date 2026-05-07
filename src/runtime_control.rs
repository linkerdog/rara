use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{AgentEvent, BashApprovalDecision};
use crate::mcp_status::{McpConnectionState, McpStatusSnapshot};
use crate::todo::TodoState;
use crate::tool::ToolOutputStream;

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
    Created { session_id: String },
    Resumed { session_id: String },
    Status { message: String },
    TurnStarted,
    TurnCancelled,
    TurnInterrupted,
    TurnFinished,
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
    Unregistered { source_id: String },
    Dropped { source_id: String, reason: String },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SkillEvent {
    Registered { source_id: String, name: String },
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
    RecordAdded { memory_id: String },
    RecordUpdated { memory_id: String },
    RecordDeleted { memory_id: String },
    SelectionUpdated,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum HookEvent {
    Declared {
        hook_id: String,
        lifecycle: HookLifecycle,
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
    RuntimeError { message: String },
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
