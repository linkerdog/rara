use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use super::super::is_compact_boundary_message;
use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentEvent, CompactState, ContentBlock, Message};
use crate::context::RetrievedMemoryCandidate;
use crate::llm::{ContextBudget, LlmBackend, LlmResponse, TokenUsage};
use crate::session::SessionManager;
use crate::state_db::{PersistedStructuredRolloutEvent, StateDb};
use crate::tool::ToolManager;
use crate::vectordb::VectorDB;
use crate::workspace::WorkspaceMemory;

struct SlowSummarizeBackend;

struct TinyBudgetSummaryBackend;

#[derive(Default)]
struct ContextWindowOnceBackend {
    summarized_lengths: Mutex<Vec<usize>>,
}

#[async_trait]
impl LlmBackend for SlowSummarizeBackend {
    async fn ask(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> Result<LlmResponse> {
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "query completed".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; 8])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        Ok("slow summary".to_string())
    }
}

#[async_trait]
impl LlmBackend for TinyBudgetSummaryBackend {
    async fn ask(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> Result<LlmResponse> {
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "query completed".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; 8])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".to_string())
    }

    fn context_budget(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> Option<ContextBudget> {
        Some(ContextBudget {
            context_window_tokens: 16,
            reserved_output_tokens: 4,
            compact_threshold_tokens: 1,
        })
    }
}

#[async_trait]
impl LlmBackend for ContextWindowOnceBackend {
    async fn ask(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> Result<LlmResponse> {
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "query completed".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; 8])
    }

    async fn summarize(&self, messages: &[Message], _instruction: &str) -> Result<String> {
        let mut summarized_lengths = self.summarized_lengths.lock().expect("lock");
        summarized_lengths.push(messages.len());
        if summarized_lengths.len() == 1 {
            return Err(crate::llm::context_window_error_for_test());
        }
        Ok("summary after dropping oldest round".to_string())
    }

    fn context_budget(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> Option<ContextBudget> {
        Some(ContextBudget {
            context_window_tokens: 16,
            reserved_output_tokens: 4,
            compact_threshold_tokens: 1,
        })
    }
}

#[tokio::test]
async fn manual_compact_replaces_older_history_with_summary() {
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("inspect the repo"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("I checked Cargo.toml and src/main.rs"),
        },
    ];

    let compacted = agent
        .compact_now_with_reporter(|_| {})
        .await
        .expect("compact should succeed");

    assert!(compacted);
    assert_eq!(agent.compact_state.compaction_count, 1);
    assert_eq!(agent.history[0].role, "system");
    let boundary = agent.history[0]
        .content
        .as_array()
        .expect("compact boundary items")
        .iter()
        .find(|item| {
            item.get("type").and_then(serde_json::Value::as_str) == Some("compact_boundary")
        })
        .expect("compact boundary");
    assert_eq!(
        boundary.get("type").and_then(serde_json::Value::as_str),
        Some("compact_boundary")
    );
    assert!(
        agent.history[1]
            .content
            .to_string()
            .contains("STRUCTURED SUMMARY OF PREVIOUS CONVERSATION")
    );
}

#[tokio::test]
async fn automatic_compaction_timeout_does_not_block_query() {
    let backend = Arc::new(SlowSummarizeBackend);
    let (_temp, session_manager, workspace, _rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        session_manager,
        workspace,
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("x".repeat(50_000)),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("y".repeat(50_000)),
        },
    ];

    let mut statuses = Vec::new();
    agent
        .query_with_mode_and_events(
            "continue".to_string(),
            crate::agent::AgentOutputMode::Silent,
            |event| {
                if let AgentEvent::Status(status) = event {
                    statuses.push(status);
                }
            },
        )
        .await
        .expect("query should continue after automatic compaction timeout");

    assert!(
        statuses
            .iter()
            .any(|status| status.contains("Automatic history compaction timed out"))
    );
    assert!(
        agent
            .history
            .last()
            .is_some_and(|message| message.content.to_string().contains("query completed"))
    );
}

#[tokio::test]
async fn automatic_compaction_failure_suspends_retry_until_history_grows() {
    let backend = Arc::new(SlowSummarizeBackend);
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("x".repeat(50_000)),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("y".repeat(50_000)),
        },
    ];

    agent
        .compact_if_needed_with_reporter(|_| {})
        .await
        .expect("automatic compaction timeout should be non-fatal");
    let after_failure = agent.compact_state.clone();
    assert_eq!(after_failure.consecutive_auto_compaction_failures, 1);
    assert!(after_failure.auto_compaction_retry_after_tokens.is_some());

    let mut statuses = Vec::new();
    agent
        .compact_if_needed_with_reporter(|event| {
            if let AgentEvent::Status(status) = event {
                statuses.push(status);
            }
        })
        .await
        .expect("suspended auto compaction should be non-fatal");

    assert!(
        statuses
            .iter()
            .any(|status| status.contains("temporarily suspended"))
    );
    assert_eq!(
        agent.compact_state.consecutive_auto_compaction_failures,
        after_failure.consecutive_auto_compaction_failures
    );
    assert_eq!(
        agent.compact_state.auto_compaction_retry_after_tokens,
        after_failure.auto_compaction_retry_after_tokens
    );
}

#[tokio::test]
async fn successful_compaction_clears_auto_failure_backoff() {
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.compact_state = CompactState {
        consecutive_auto_compaction_failures: 2,
        auto_compaction_retry_after_tokens: Some(100_000),
        ..Default::default()
    };
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("inspect the repo"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("I checked Cargo.toml and src/main.rs"),
        },
    ];

    let compacted = agent
        .compact_now_with_reporter(|_| {})
        .await
        .expect("manual compaction should succeed");

    assert!(compacted);
    assert_eq!(agent.compact_state.consecutive_auto_compaction_failures, 0);
    assert_eq!(agent.compact_state.auto_compaction_retry_after_tokens, None);
}

#[tokio::test]
async fn compaction_retries_context_window_error_by_dropping_oldest_api_round() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let backend = Arc::new(ContextWindowOnceBackend::default());
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("old user"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("old answer"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"fn main() {}"}
            ]),
        },
    ];

    let mut statuses = Vec::new();
    agent
        .compact_if_needed_with_reporter(|event| {
            if let AgentEvent::Status(status) = event {
                statuses.push(status);
            }
        })
        .await
        .expect("context-window retry should allow compaction to succeed");

    assert_eq!(
        *backend.summarized_lengths.lock().expect("lock"),
        vec![4, 3],
        "retry should drop exactly the oldest API round from the compact input"
    );
    assert!(
        statuses
            .iter()
            .any(|status| status.contains("retrying without the oldest API round"))
    );
    assert_eq!(agent.compact_state.compaction_count, 1);
    assert!(agent.history.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("summary after dropping oldest round")
    }));
}

#[tokio::test]
async fn manual_compact_carries_recent_files_forward() {
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.history = vec![
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs"}},
                {"type":"tool_use","id":"tool-2","name":"list_files","input":{"path":"src/agent"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"fn main() {}"},
                {"type":"tool_result","tool_use_id":"tool-2","content":"planning.rs"}
            ]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("I inspected the relevant files."),
        },
    ];

    let compacted = agent
        .compact_now_with_reporter(|_| {})
        .await
        .expect("compact should succeed");

    assert!(compacted);
    assert_eq!(agent.history[2].role, "system");
    assert_eq!(agent.history[3].role, "system");
    let boundary = agent.history[0]
        .content
        .as_array()
        .expect("compact boundary items")
        .iter()
        .find(|item| {
            item.get("type").and_then(serde_json::Value::as_str) == Some("compact_boundary")
        })
        .expect("compact boundary");
    assert_eq!(
        boundary
            .get("recent_file_count")
            .and_then(serde_json::Value::as_u64),
        Some(2)
    );
    let recent_files = agent.history[2].content.to_string();
    assert!(recent_files.contains("RECENT FILES FROM COMPACTED HISTORY"));
    assert!(recent_files.contains("src/main.rs"));
    assert!(recent_files.contains("src/agent"));
    let excerpts = agent.history[3].content.to_string();
    assert!(excerpts.contains("RECENT FILE EXCERPTS FROM COMPACTED HISTORY"));
    assert!(excerpts.contains("### src/main.rs"));
    assert!(excerpts.contains("fn main() {}"));
    let context = agent.shared_runtime_context();
    let kinds = context
        .compaction
        .source_entries
        .iter()
        .map(|entry| entry.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            "compact_boundary",
            "compacted_summary",
            "recent_files",
            "recent_file_excerpts"
        ]
    );
}

#[test]
fn compact_boundary_detection_handles_typed_array_shape() {
    let message = Message {
        role: "system".to_string(),
        content: json!([
            {
                "type": "text",
                "text": "Previous conversation compacted."
            },
            {
                "type": "compact_boundary",
                "boundary_version": 1,
                "before_tokens": 100
            }
        ]),
    };

    assert!(is_compact_boundary_message(&message));
}

#[tokio::test]
async fn manual_compact_carries_retrieved_memory_forward() {
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("continue from prior memory"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("I found the relevant project memory."),
        },
    ];
    agent.retrieved_memory_candidates = vec![RetrievedMemoryCandidate {
        kind: "retrieved_workspace_memory".to_string(),
        label: "Project Path".to_string(),
        detail: "The stable project path is /tmp/rara.".to_string(),
        selection_reason: "ranked from memory search".to_string(),
        rank: 0,
    }];

    let compacted = agent
        .compact_now_with_reporter(|_| {})
        .await
        .expect("compact should succeed");

    assert!(compacted);
    let context = agent.shared_runtime_context();
    let memory_entry = context
        .compaction
        .source_entries
        .iter()
        .find(|entry| entry.kind == "compacted_memory")
        .expect("memory carry-over entry");
    assert_eq!(memory_entry.source_descriptor, "history.compaction.memory");
    assert!(
        memory_entry
            .detail
            .contains("The stable project path is /tmp/rara.")
    );
    assert!(
        context
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .any(|item| item.kind == "compacted_memory"
                && item
                    .detail
                    .contains("The stable project path is /tmp/rara."))
    );
}

#[tokio::test]
async fn manual_compact_carries_invoked_skill_forward() {
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("review the current change"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {
                    "type": "tool_use",
                    "id": "skill-1",
                    "name": "skill",
                    "input": {
                        "action": "invoke",
                        "skill_name": "review",
                        "args": "PR 226"
                    }
                }
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {
                    "type": "tool_result",
                    "tool_use_id": "skill-1",
                    "content": json!({
                        "name": "review",
                        "title": "Review",
                        "scope": "repo",
                        "display_path": ".agents/skills/review/SKILL.md",
                        "instructions": "Check correctness, tests, and review comments before approving."
                    })
                    .to_string()
                }
            ]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("I will follow the review workflow."),
        },
    ];

    let compacted = agent
        .compact_now_with_reporter(|_| {})
        .await
        .expect("compact should succeed");

    assert!(compacted);
    let context = agent.shared_runtime_context();
    let skill_entry = context
        .compaction
        .source_entries
        .iter()
        .find(|entry| entry.kind == "compacted_skills")
        .expect("skill carry-over entry");
    assert_eq!(skill_entry.source_descriptor, "history.compaction.skills");
    assert!(skill_entry.detail.contains("review"));
    assert!(
        skill_entry
            .detail
            .contains(".agents/skills/review/SKILL.md")
    );
    assert!(
        context
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .any(|item| item.kind == "compacted_skills" && item.detail.contains("review"))
    );
}

#[tokio::test]
async fn manual_compact_carries_hook_and_mcp_retain_hints_forward() {
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("continue with hook and mcp context"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {
                    "type": "compaction_retain_hint",
                    "label": "Post-tool hook",
                    "source_descriptor": "hook.post_tool_use.validation",
                    "detail": "Validation hook found generated files that still need tests.",
                    "inclusion_reason": "hook output should survive compaction"
                },
                {
                    "type": "compaction_retain_hint",
                    "label": "Repository docs resource",
                    "source_descriptor": "mcp.resource.repo_docs",
                    "detail": "MCP docs resource selected the local thread-store schema notes.",
                    "inclusion_reason": "mcp resource should survive compaction"
                },
                {
                    "type": "compaction_retain_hint",
                    "label": "Ignored",
                    "source_descriptor": "workspace.memory",
                    "detail": "This is not a hook or MCP source."
                }
            ]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("I will keep these runtime sources in view."),
        },
    ];

    let compacted = agent
        .compact_now_with_reporter(|_| {})
        .await
        .expect("compact should succeed");

    assert!(compacted);
    let context = agent.shared_runtime_context();
    let hook_entry = context
        .compaction
        .source_entries
        .iter()
        .find(|entry| entry.kind == "compacted_hooks")
        .expect("hook carry-over entry");
    assert_eq!(hook_entry.source_descriptor, "history.compaction.hooks");
    assert!(
        hook_entry
            .detail
            .contains("Validation hook found generated files")
    );
    let mcp_entry = context
        .compaction
        .source_entries
        .iter()
        .find(|entry| entry.kind == "compacted_mcp")
        .expect("mcp carry-over entry");
    assert_eq!(mcp_entry.source_descriptor, "history.compaction.mcp");
    assert!(
        mcp_entry
            .detail
            .contains("MCP docs resource selected the local thread-store schema notes")
    );
    assert!(
        context
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .any(|item| item.kind == "compacted_hooks")
    );
    assert!(
        context
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .any(|item| item.kind == "compacted_mcp")
    );
}

#[tokio::test]
async fn manual_compact_prefers_latest_excerpt_and_tracks_apply_patch() {
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.history = vec![
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs","start_line":1,"end_line":2}},
                {"type":"tool_use","id":"tool-2","name":"apply_patch","input":{"path":"src/lib.rs"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"old snippet"},
                {"type":"tool_result","tool_use_id":"tool-2","content":"patch applied"}
            ]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-3","name":"read_file","input":{"path":"src/main.rs","offset":10,"limit":3}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-3","content":"new snippet"}
            ]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("I updated the inspection notes."),
        },
    ];

    let compacted = agent
        .compact_now_with_reporter(|_| {})
        .await
        .expect("compact should succeed");

    assert!(compacted);
    let recent_files = agent.history[2].content.to_string();
    assert!(recent_files.contains("src/lib.rs"));
    let excerpts = agent.history[3].content.to_string();
    assert!(excerpts.contains("new snippet"));
    assert!(!excerpts.contains("old snippet"));
    assert!(excerpts.contains("lines 10-12"));
}

#[tokio::test]
async fn partial_compact_replaces_only_selected_api_round_range() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager.clone(),
        workspace,
    );
    agent.session_id = "partial-compact".to_string();
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("keep before"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("old answer"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/old.rs"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"old output"}
            ]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!("keep after"),
        },
    ];

    let compacted = agent
        .compact_range_now_with_reporter(1, 4, |_| {})
        .await
        .expect("partial compact should succeed");

    assert!(compacted);
    assert_eq!(agent.history[0].content, json!("keep before"));
    assert!(
        agent
            .history
            .iter()
            .any(|message| message.content.to_string().contains("STRUCTURED SUMMARY"))
    );
    assert_eq!(
        agent.history.last().expect("last").content,
        json!("keep after")
    );
    let events = session_manager
        .load_compaction_events("partial-compact")
        .expect("compaction events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].replaced_start, Some(1));
    assert_eq!(events[0].replaced_end, Some(4));

    let state_db = StateDb::new_for_root_dir(rara_dir).expect("state db");
    let rollout_events = state_db
        .load_rollout_events("partial-compact")
        .expect("rollout events");
    assert!(rollout_events.iter().any(|event| matches!(
        event,
        PersistedStructuredRolloutEvent::Compaction {
            replaced_start,
            replaced_end,
            metadata_owner,
            ..
        } if *replaced_start == Some(1)
            && *replaced_end == Some(4)
            && metadata_owner.as_deref() == Some("runtime.compaction")
    )));
}

#[tokio::test]
async fn partial_compact_rejects_non_api_round_boundary_range() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let backend = Arc::new(SequencedBackend::new(Vec::new()));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("start"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/old.rs"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"old output"}
            ]),
        },
    ];

    let err = agent
        .compact_range_now_with_reporter(2, 3, |_| {})
        .await
        .expect_err("range splitting an API round should fail");

    assert!(
        err.to_string()
            .contains("partial compaction range must align")
    );
}

#[tokio::test]
async fn manual_compact_preserves_recent_api_round_pair() {
    let backend = Arc::new(TinyBudgetSummaryBackend);
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.history = vec![
        Message {
            role: "user".to_string(),
            content: json!("inspect old state"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-old","name":"read_file","input":{"path":"src/old.rs"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-old","content":"old output"}
            ]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-recent","name":"read_file","input":{"path":"src/recent.rs"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-recent","content":"recent output"}
            ]),
        },
    ];

    let compacted = agent
        .compact_now_with_reporter(|_| {})
        .await
        .expect("compact should succeed");

    assert!(compacted);
    let recent_tool_use_index = agent
        .history
        .iter()
        .position(|message| message.content.to_string().contains("tool-recent"))
        .expect("recent tool use should be retained");
    assert_eq!(agent.history[recent_tool_use_index].role, "assistant");
    assert_eq!(
        agent.history[recent_tool_use_index + 1].role,
        "user",
        "tool result should stay with the retained assistant API round"
    );
    assert!(
        agent.history[recent_tool_use_index + 1]
            .content
            .to_string()
            .contains("tool-recent")
    );
}
