use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProvenance {
    pub controller: RuntimeControllerKind,
    pub adapter: Option<String>,
    pub session_id: Option<String>,
    pub source_id: Option<String>,
    pub trust: RuntimeSourceTrust,
    pub authorship: RuntimeSourceAuthorship,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceTrust {
    Trusted,
    Untrusted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceAuthorship {
    UserProvided,
    Generated,
    Runtime,
}

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeControlEnvelope {
    pub request_id: String,
    pub provenance: RuntimeProvenance,
    pub request: RuntimeControlRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SessionControlRequest {
    CreateSession,
    ResumeSession { session_id: String },
    CancelCurrentTurn,
    InterruptCurrentTurn,
    QueryRuntimeState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InputControlRequest {
    SubmitUserPrompt {
        prompt: String,
    },
    AnswerPendingInput {
        answer: String,
    },
    AnswerPlanApproval {
        decision: PlanApprovalDecision,
        feedback: Option<String>,
    },
    AnswerShellApproval {
        decision: ShellApprovalDecision,
    },
    SubmitFollowUp {
        prompt: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanApprovalDecision {
    Approve,
    ContinuePlanning,
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellApprovalDecision {
    Once,
    Prefix,
    Always,
    Suggestion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum OutputSubscriptionRequest {
    Subscribe { subscriber_id: String },
    Unsubscribe { subscriber_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PromptSourceLifetime {
    Turns(u32),
    Session,
    Persistent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSourceRegistration {
    pub source_id: String,
    pub scope: SourceScope,
    pub layer: SourceLayer,
    pub budget_hint_tokens: Option<u32>,
    pub lifetime: PromptSourceLifetime,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    Home,
    Repo,
    CurrentWorkingDirectory,
    Session,
    Protocol,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLayer {
    System,
    Developer,
    User,
    Memory,
    Skill,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PromptSourceControlRequest {
    Register(PromptSourceRegistration),
    Unregister { source_id: String },
    QuerySources,
}

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum McpControlRequest {
    QueryStatus,
    Refresh { server_name: Option<String> },
    Reconnect { server_name: String },
}

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Thread,
    Workspace,
}

/// Lifecycle phase for hooks declared through the app-server control plane.
pub use rara_instructions::HookLifecycle;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ApprovalControlRequest {
    AnswerPendingApproval { approval_id: String, approved: bool },
    QueryPendingApprovals,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
}
