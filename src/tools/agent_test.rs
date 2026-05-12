use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use rara_memory::vectordb::VectorDB;
use rara_persistence::thread_data::PersistedStructuredRolloutEvent;
use rara_persistence::thread_rollout_log;
use rara_state::state_db::StateDb;
use rara_tools::tool::{Tool, ToolCallContext, ToolError};
use serde_json::json;
use tempfile::tempdir;
use tokio::time::{Duration, sleep};

use super::{
    AgentDefinition, BACKGROUND_SUBAGENT_COMPLETED_RETENTION, BackgroundSubAgentRecord,
    BackgroundSubAgentStore, SubAgentKind, SubagentProgress, TEAM_CREATE_CONCURRENCY_LIMIT,
    append_subagent_prompt, build_filtered_tool_manager, build_read_only_tool_manager,
    build_subagent_tool_manager, latest_assistant_text_from_history, parse_team_task_kind,
    resolve_kind_definition, resolve_spawn_agent_definition,
};
use crate::agent::Message;
use crate::llm::{ContentBlock, EmbeddingBackend, LlmBackend, LlmResponse, MockLlm};
use crate::prompt::PromptRuntimeConfig;
use crate::session::SessionManager;
use crate::session_transcript::{load_transcript, model_visible_messages};
use crate::thread_store::{ThreadMetadataSource, ThreadStore};
use crate::tools::agent::{
    AgentTool, ExploreAgentTool, PlanAgentTool, SubAgentResumeTool, SubAgentStopTool,
    TeamCreateTool,
};
use crate::workspace::WorkspaceMemory;

struct CountingBackend {
    calls: Arc<AtomicUsize>,
}

struct PeakBackend {
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

struct PlanStateBackend;

struct SlowBackend;

fn mock_embedding_backend() -> Arc<dyn EmbeddingBackend> {
    Arc::new(MockLlm)
}

fn record_peak(current: usize, peak: &AtomicUsize) {
    let mut observed = peak.load(Ordering::SeqCst);
    while current > observed {
        match peak.compare_exchange(observed, current, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(next) => observed = next,
        }
    }
}

#[async_trait]
impl LlmBackend for CountingBackend {
    async fn ask(
        &self,
        messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let last = messages
            .last()
            .and_then(|message| message.content.as_str())
            .unwrap_or_default();
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: format!("counted {last}"),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: None,
        })
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 4])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        Ok("summary".to_string())
    }
}

#[async_trait]
impl LlmBackend for PlanStateBackend {
    async fn ask(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Inspect subagent restore\n- [in_progress] Persist child state\n</proposed_plan>\nPlan state ready.".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            })
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 4])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        Ok("summary".to_string())
    }
}

#[async_trait]
impl LlmBackend for PeakBackend {
    async fn ask(
        &self,
        messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        record_peak(current, &self.peak);
        sleep(Duration::from_millis(50)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let last = messages
            .last()
            .and_then(|message| message.content.as_str())
            .unwrap_or_default();
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: format!("peak {last}"),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: None,
        })
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 4])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        Ok("summary".to_string())
    }
}

#[async_trait]
impl LlmBackend for SlowBackend {
    async fn ask(
        &self,
        messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        sleep(Duration::from_millis(250)).await;
        let last = messages
            .last()
            .and_then(|message| message.content.as_str())
            .unwrap_or_default();
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: format!("slow {last}"),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: None,
        })
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 4])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        Ok("summary".to_string())
    }
}

#[test]
fn read_only_subagent_manager_excludes_mutating_and_agent_tools() {
    let manager = build_read_only_tool_manager();
    assert!(manager.get_tool("read_file").is_some());
    assert!(manager.get_tool("list_files").is_some());
    assert!(manager.get_tool("glob").is_some());
    assert!(manager.get_tool("grep").is_some());
    assert!(manager.get_tool("search_files").is_none());
    assert!(manager.get_tool("write_file").is_none());
    assert!(manager.get_tool("apply_patch").is_none());
    assert!(manager.get_tool("bash").is_none());
    assert!(manager.get_tool("background_task_list").is_none());
    assert!(manager.get_tool("background_task_status").is_none());
    assert!(manager.get_tool("background_task_stop").is_none());
    assert!(manager.get_tool("pty_start").is_none());
    assert!(manager.get_tool("pty_list").is_none());
    assert!(manager.get_tool("pty_status").is_none());
    assert!(manager.get_tool("pty_stop").is_none());
    assert!(manager.get_tool("spawn_agent").is_none());
    assert!(manager.get_tool("explore_agent").is_none());
    assert!(manager.get_tool("plan_agent").is_none());
    assert!(manager.get_tool("team_create").is_none());
}

#[test]
fn general_subagent_manager_does_not_expose_recursive_agent_tools() {
    let manager = build_subagent_tool_manager(SubAgentKind::General);
    assert!(manager.get_tool("spawn_agent").is_none());
    assert!(manager.get_tool("explore_agent").is_none());
    assert!(manager.get_tool("plan_agent").is_none());
    assert!(manager.get_tool("team_create").is_none());
    assert!(manager.get_tool("bash").is_none());
    assert!(manager.get_tool("pty_start").is_none());
}

#[test]
fn append_subagent_prompt_preserves_existing_append_prompt() {
    let runtime = PromptRuntimeConfig {
        append_system_prompt: Some("existing tail".to_string()),
        ..Default::default()
    };
    let updated = append_subagent_prompt(runtime, "sub-agent");
    assert_eq!(
        updated.append_system_prompt.as_deref(),
        Some("existing tail\n\nsub-agent")
    );
}

#[test]
fn subagent_prompt_requires_instruction_constraints_and_workspace_boundary() {
    let prompt = SubAgentKind::Explore.append_prompt();

    assert!(prompt.contains("Treat the assigned instruction as the complete task contract."));
    assert!(prompt.contains("Honor every constraint in the assigned instruction"));
    assert!(prompt.contains("Stay inside the current workspace"));
}

#[test]
fn general_subagent_prompt_declares_no_tool_access() {
    let prompt = SubAgentKind::General.append_prompt();

    assert!(prompt.contains("no-tool reasoning sub-agent"));
    assert!(prompt.contains("do not have repository, shell, editing, patching, or browser tools"));
    assert!(prompt.contains("answer only from the provided instruction/context"));
}

#[test]
fn read_only_subagent_prompts_forbid_mutation_and_shell_workarounds() {
    for kind in [SubAgentKind::Explore, SubAgentKind::Plan] {
        let prompt = kind.append_prompt();

        assert!(prompt.contains("STRICT READ-ONLY"));
        assert!(prompt.contains("creating, modifying, deleting, moving, or copying files"));
        assert!(prompt.contains("including /tmp"));
        assert!(prompt.contains("redirection"));
        assert!(prompt.contains("Bash, PTY, editing, patching"));
        assert!(prompt.contains("read_file, list_files, glob, grep"));
        assert!(prompt.contains("instead of attempting a workaround"));
    }

    assert!(
        !SubAgentKind::General
            .append_prompt()
            .contains("STRICT READ-ONLY")
    );
}

#[test]
fn latest_assistant_text_supports_string_content() {
    let history = vec![Message {
        role: "assistant".into(),
        content: json!("plain string assistant content"),
    }];

    assert_eq!(
        latest_assistant_text_from_history(&history).as_deref(),
        Some("plain string assistant content")
    );
}

#[test]
fn team_task_kind_defaults_to_explore_and_rejects_unknown_values() {
    assert!(matches!(
        parse_team_task_kind(0, None).unwrap(),
        SubAgentKind::Explore
    ));
    assert!(matches!(
        parse_team_task_kind(0, Some("general")).unwrap(),
        SubAgentKind::General
    ));
    assert!(matches!(
        parse_team_task_kind(0, Some("plan")).unwrap(),
        SubAgentKind::Plan
    ));
    let err = parse_team_task_kind(3, Some("unknown")).expect_err("invalid kind");
    assert!(matches!(err, ToolError::InvalidInput(message) if message.contains("tasks[3].kind")));
}

#[tokio::test]
async fn team_create_runs_real_subagents_in_order() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(MockLlm),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
    };

    let result = tool
        .call(json!({
            "tasks": [
                {
                    "name": "research",
                    "kind": "general",
                    "instruction": "summarize one"
                },
                {
                    "name": "inspect",
                    "kind": "explore",
                    "instruction": "summarize two"
                }
            ]
        }))
        .await
        .expect("team_create result");
    let results = result["team_results"]
        .as_array()
        .expect("team_results array");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["name"], "research");
    assert_eq!(results[0]["status"], "done");
    assert_eq!(results[0]["summary"], "Mock Response: summarize one");
    assert_eq!(results[1]["name"], "inspect");
    assert_eq!(results[1]["status"], "explored");
    assert_eq!(results[1]["summary"], "Mock Response: summarize two");
    assert_ne!(results[0]["status"], "mocked_done");
}

#[tokio::test]
async fn team_create_validates_all_tasks_before_running_subagents() {
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(CountingBackend {
            calls: calls.clone(),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
    };

    let err = tool
        .call(json!({
            "tasks": [
                {
                    "name": "valid",
                    "instruction": "should not run"
                },
                {
                    "name": "invalid"
                }
            ]
        }))
        .await
        .expect_err("invalid task");

    assert!(matches!(err, ToolError::InvalidInput(message) if message == "tasks[1].instruction"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn team_create_rejects_non_string_kind_before_running_subagents() {
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(CountingBackend {
            calls: calls.clone(),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
    };

    let err = tool
        .call(json!({
            "tasks": [
                {
                    "instruction": "should not run",
                    "kind": 1
                }
            ]
        }))
        .await
        .expect_err("invalid kind");

    assert!(
        matches!(err, ToolError::InvalidInput(message) if message == "tasks[0].kind must be a string")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn team_create_rejects_unstable_explicit_name_before_running_subagents() {
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(CountingBackend {
            calls: calls.clone(),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
    };

    let err = tool
        .call(json!({
            "tasks": [
                {
                    "name": "!!!",
                    "instruction": "should not run"
                }
            ]
        }))
        .await
        .expect_err("invalid name");

    assert!(matches!(err, ToolError::InvalidInput(message)
                if message == "tasks[0].name must normalize to a non-empty agent id label"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn spawn_agent_rejects_name_that_normalizes_empty_before_running_subagent() {
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = AgentTool {
        backend: Arc::new(CountingBackend {
            calls: calls.clone(),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
    };

    let err = tool
        .call(json!({
            "name": "!!!",
            "instruction": "should not run"
        }))
        .await
        .expect_err("invalid name");

    assert!(matches!(err, ToolError::InvalidInput(message)
                if message == "name must normalize to a non-empty agent id label"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn team_create_limits_concurrent_subagents() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(PeakBackend {
            in_flight,
            peak: peak.clone(),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
    };
    let tasks = (0..8)
        .map(|idx| json!({ "kind": "general", "instruction": format!("task {idx}") }))
        .collect::<Vec<_>>();

    let result = tool
        .call(json!({ "tasks": tasks }))
        .await
        .expect("team_create result");

    assert_eq!(result["team_results"].as_array().expect("results").len(), 8);
    let observed_peak = peak.load(Ordering::SeqCst);
    assert!(observed_peak <= TEAM_CREATE_CONCURRENCY_LIMIT);
    assert!(observed_peak > 1);
}

#[tokio::test]
async fn team_create_writes_parent_scoped_sidechain_transcripts() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({
                "tasks": [
                    {
                        "name": "Review Worker",
                        "kind": "general",
                        "instruction": "summarize this task"
                    }
                ]
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("team_create result");

    let item = &result["team_results"][0];
    let agent_id = item["agent_id"].as_str().expect("agent_id");
    let child_session_id = item["session_id"].as_str().expect("session_id");
    let result_status = item["status"].as_str().expect("status");
    let result_summary = item["summary"].as_str().expect("summary");
    assert!(agent_id.starts_with("review-worker-"));
    let transcript_path = rara_dir
        .join("rollouts")
        .join("parent-session")
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    let transcript = load_transcript(&transcript_path).expect("transcript");

    assert_eq!(transcript.parse_errors, 0);
    assert!(model_visible_messages(&transcript.entries).is_empty());
    assert!(matches!(
        &transcript.entries[0],
        crate::session_transcript::SessionTranscriptEntry::SessionMeta {
            session_id,
            parent_session_id: Some(parent),
            agent_id: Some(entry_agent_id),
            is_sidechain: true,
            ..
        } if session_id == child_session_id
            && parent == "parent-session"
            && entry_agent_id == agent_id
    ));

    let events =
        thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
            .expect("rollout events");
    assert!(matches!(
        events.as_slice(),
        [PersistedStructuredRolloutEvent::SpawnAgent {
            event_id,
            agent_id: event_agent_id,
            name: Some(name),
            child_session_id: event_child_session_id,
            status,
            summary: Some(summary),
            ..
        }] if event_id.starts_with("spawn-")
            && event_agent_id == agent_id
            && name == "Review Worker"
            && event_child_session_id == child_session_id
            && status == result_status
            && summary == result_summary
    ));
}

#[tokio::test]
async fn spawn_agent_writes_parent_scoped_sidechain_transcript() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = AgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({
                "name": "General Worker",
                "instruction": "summarize this task"
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("spawn_agent result");

    let agent_id = result["agent_id"].as_str().expect("agent_id");
    let child_session_id = result["session_id"].as_str().expect("session_id");
    assert!(agent_id.starts_with("general-worker-"));
    let transcript_path = rara_dir
        .join("rollouts")
        .join("parent-session")
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    let transcript = load_transcript(&transcript_path).expect("transcript");
    assert_eq!(transcript.parse_errors, 0);
    assert!(model_visible_messages(&transcript.entries).is_empty());

    let events =
        thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
            .expect("rollout events");
    assert!(matches!(
        events.as_slice(),
        [PersistedStructuredRolloutEvent::SpawnAgent {
            agent_id: event_agent_id,
            name: Some(name),
            child_session_id: event_child_session_id,
            status,
            ..
        }] if event_agent_id == agent_id
            && name == "General Worker"
            && event_child_session_id == child_session_id
            && status == "done"
    ));

    let state_db = StateDb::new_for_root_dir(rara_dir).expect("state db");
    let thread_store = ThreadStore::new(tool.session_manager.as_ref(), &state_db);
    let child = thread_store
        .load_thread(child_session_id)
        .expect("child thread");
    assert_eq!(
        child.provenance.metadata_source,
        ThreadMetadataSource::StructuredMetadata
    );
    assert_eq!(child.metadata.origin_kind, "subagent");
    assert_eq!(
        child.metadata.forked_from_thread_id.as_deref(),
        Some("parent-session")
    );
    assert!(!child.history.is_empty());
}

#[tokio::test]
async fn background_subagent_resume_returns_completed_summary_without_inline_sidechain() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let background_subagents = Arc::new(BackgroundSubAgentStore::default());
    let session_manager =
        Arc::new(SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"));
    let tool = ExploreAgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: background_subagents.clone(),
    };
    let resume = SubAgentResumeTool {
        background_subagents,
    };

    let mut progress = |_| {};
    let started = tool
        .call_with_context_events(
            json!({
                "instruction": "inspect this in the background",
                "run_in_background": true
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("background start");
    let agent_id = started["agent_id"].as_str().expect("agent_id");
    let child_session_id = started["session_id"].as_str().expect("session_id");
    assert_eq!(started["status"], "running");

    let mut completed = None;
    for _ in 0..20 {
        let status = resume
            .call(json!({ "agent_id": agent_id }))
            .await
            .expect("resume status");
        if status["status"] != "running" {
            completed = Some(status);
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    let completed = completed.expect("background sub-agent completed");
    assert_eq!(completed["status"], "explored");
    assert!(
        completed["summary"]
            .as_str()
            .expect("summary")
            .starts_with("counted")
    );
    assert_eq!(completed["session_id"], child_session_id);

    let transcript_path = rara_dir
        .join("rollouts")
        .join("parent-session")
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    let transcript = load_transcript(&transcript_path).expect("transcript");
    assert_eq!(transcript.parse_errors, 0);
    assert!(model_visible_messages(&transcript.entries).is_empty());
}

#[tokio::test]
async fn background_subagent_stop_marks_running_task_cancelled() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let background_subagents = Arc::new(BackgroundSubAgentStore::default());
    let tool = ExploreAgentTool {
        backend: Arc::new(SlowBackend),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: background_subagents.clone(),
    };
    let stop = SubAgentStopTool {
        background_subagents: background_subagents.clone(),
    };
    let resume = SubAgentResumeTool {
        background_subagents,
    };

    let mut progress = |_| {};
    let started = tool
        .call_with_context_events(
            json!({
                "instruction": "keep running until stopped",
                "run_in_background": true
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("background start");
    let agent_id = started["agent_id"].as_str().expect("agent_id");

    let stopped = stop
        .call(json!({ "agent_id": agent_id }))
        .await
        .expect("stop sub-agent");
    assert_eq!(stopped["status"], "cancelled");

    let resumed = resume
        .call(json!({ "agent_id": agent_id }))
        .await
        .expect("resume cancelled sub-agent");
    assert_eq!(resumed["status"], "cancelled");
    assert!(resumed["finished_at"].as_u64().is_some());
}

#[test]
fn background_subagent_store_prunes_old_completed_records() {
    let store = BackgroundSubAgentStore::default();
    {
        let mut inner = store.inner.lock().expect("store");
        for idx in 0..(BACKGROUND_SUBAGENT_COMPLETED_RETENTION + 3) {
            let agent_id = format!("agent-{idx}");
            inner.tasks.insert(
                agent_id.clone(),
                BackgroundSubAgentRecord {
                    agent_id,
                    session_id: format!("session-{idx}"),
                    name: None,
                    model: None,
                    progress: SubagentProgress::new("test".to_string()),
                    kind: "general",
                    parent_session_id: None,
                    status: "done",
                    summary: Some(format!("summary {idx}")),
                    error: None,
                    persistence_error: None,
                    plan: None,
                    plan_explanation: None,
                    request_user_input: None,
                    started_at: idx as u64,
                    finished_at: Some(idx as u64),
                },
            );
        }
    }

    store.finish(
        &format!("agent-{}", BACKGROUND_SUBAGENT_COMPLETED_RETENTION + 2),
        Err(ToolError::ExecutionFailed("refresh latest".to_string())),
    );

    let records = store.list().expect("records");
    assert_eq!(records.len(), BACKGROUND_SUBAGENT_COMPLETED_RETENTION);
    assert!(records.iter().any(|record| record.agent_id == "agent-66"));
    assert!(!records.iter().any(|record| record.agent_id == "agent-0"));
}

#[tokio::test]
async fn background_plan_agent_resume_returns_plan_state() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let background_subagents = Arc::new(BackgroundSubAgentStore::default());
    let tool = PlanAgentTool {
        backend: Arc::new(PlanStateBackend),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, temp.path().join(".rara"))),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: background_subagents.clone(),
    };
    let resume = SubAgentResumeTool {
        background_subagents,
    };

    let mut progress = |_| {};
    let started = tool
        .call_with_context_events(
            json!({
                "instruction": "plan this in the background",
                "run_in_background": true
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("background start");
    let agent_id = started["agent_id"].as_str().expect("agent_id");

    let mut completed = None;
    for _ in 0..20 {
        let status = resume
            .call(json!({ "agent_id": agent_id }))
            .await
            .expect("resume status");
        if status["status"] != "running" {
            completed = Some(status);
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    let completed = completed.expect("background plan sub-agent completed");
    assert_eq!(completed["status"], "planned");
    let steps = completed["plan"].as_array().expect("plan steps");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["step"], "Inspect subagent restore");
    assert_eq!(steps[1]["status"], "in_progress");
}

#[tokio::test]
async fn plan_agent_writes_parent_scoped_sidechain_transcript() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = PlanAgentTool {
        backend: Arc::new(PlanStateBackend),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({ "instruction": "plan this task" }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("plan_agent result");

    let agent_id = result["agent_id"].as_str().expect("agent_id");
    let child_session_id = result["session_id"].as_str().expect("session_id");
    assert!(agent_id.starts_with("plan-"));
    let transcript_path = rara_dir
        .join("rollouts")
        .join("parent-session")
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    let transcript = load_transcript(&transcript_path).expect("transcript");
    assert_eq!(transcript.parse_errors, 0);
    assert!(model_visible_messages(&transcript.entries).is_empty());

    let events =
        thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
            .expect("rollout events");
    assert!(matches!(
        events.as_slice(),
        [PersistedStructuredRolloutEvent::SpawnAgent {
            agent_id: event_agent_id,
            name: None,
            child_session_id: event_child_session_id,
            status,
            ..
        }] if event_agent_id == agent_id
            && event_child_session_id == child_session_id
            && status == "planned"
    ));

    let state_db = StateDb::new_for_root_dir(rara_dir).expect("state db");
    let thread_store = ThreadStore::new(tool.session_manager.as_ref(), &state_db);
    let child = thread_store
        .load_thread(child_session_id)
        .expect("child thread");
    assert_eq!(
        child.provenance.metadata_source,
        ThreadMetadataSource::StructuredMetadata
    );
    assert_eq!(child.plan_steps.len(), 2);
    assert_eq!(child.plan_steps[0].step, "Inspect subagent restore");
    assert_eq!(child.plan_steps[0].status, "pending");
    assert_eq!(child.plan_steps[1].step, "Persist child state");
    assert_eq!(child.plan_steps[1].status, "in_progress");
}

#[tokio::test]
async fn subagent_without_parent_context_does_not_write_sidechain() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = ExploreAgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
    };

    let result = tool
        .call(json!({ "instruction": "look around" }))
        .await
        .expect("explore result");

    assert!(
        result["agent_id"]
            .as_str()
            .expect("agent_id")
            .starts_with("explore-")
    );
    assert!(!rara_dir.join("rollouts").join("subagents").exists());
    assert!(
        thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
            .expect("rollout events")
            .is_empty()
    );
}

#[tokio::test]
async fn subagent_returns_result_when_sidechain_persistence_fails() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let rollouts_dir = rara_dir.join("rollouts");
    std::fs::create_dir_all(&rollouts_dir).expect("rollouts");
    std::fs::write(rollouts_dir.join("blocked-parent"), b"not a directory").expect("blocking file");
    let tool = ExploreAgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({ "instruction": "look around" }),
            ToolCallContext::default().with_session_id("blocked-parent"),
            &mut progress,
        )
        .await
        .expect("explore result");

    assert_eq!(result["status"], "explored");
    assert!(
        result["summary"]
            .as_str()
            .expect("summary")
            .starts_with("counted")
    );
    assert!(
        !result["persistence_error"]
            .as_str()
            .expect("persistence error")
            .is_empty()
    );
}

#[tokio::test]
async fn team_create_rejects_too_many_tasks() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(MockLlm),
        embedding_backend: mock_embedding_backend(),
        vdb: Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
    };

    let tasks = (0..9)
        .map(|idx| json!({ "instruction": format!("task {idx}") }))
        .collect::<Vec<_>>();
    let err = tool
        .call(json!({ "tasks": tasks }))
        .await
        .expect_err("too many tasks");

    assert!(matches!(err, ToolError::InvalidInput(message) if message.contains("at most 8 items")));
}

#[test]
fn tool_manager_retain_filters_tools_by_name() {
    let mut manager = build_read_only_tool_manager();
    assert!(manager.get_tool("grep").is_some());
    assert!(manager.get_tool("glob").is_some());
    manager.retain(|name| name == "grep");
    assert!(manager.get_tool("grep").is_some());
    assert!(manager.get_tool("glob").is_none());
}

#[test]
fn filtered_tool_manager_respects_tools_whitelist() {
    let definition = AgentDefinition {
        name: "custom".into(),
        description: "custom".into(),
        tools: vec!["Grep".into(), "Read".into()],
        disallowed_tools: vec![],
        model: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    };
    let manager = build_filtered_tool_manager(SubAgentKind::Explore, &definition);
    assert!(manager.get_tool("grep").is_some());
    assert!(manager.get_tool("read_file").is_some());
    assert!(manager.get_tool("glob").is_none());
    assert!(manager.get_tool("list_files").is_none());
}

#[test]
fn filtered_tool_manager_respects_disallowed_tools_blacklist() {
    let definition = AgentDefinition {
        name: "custom".into(),
        description: "custom".into(),
        tools: vec![],
        disallowed_tools: vec!["Grep".into(), "Glob".into()],
        model: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    };
    let manager = build_filtered_tool_manager(SubAgentKind::Explore, &definition);
    assert!(manager.get_tool("read_file").is_some());
    assert!(manager.get_tool("list_files").is_some());
    assert!(manager.get_tool("grep").is_none());
    assert!(manager.get_tool("glob").is_none());
}

#[test]
fn filtered_tool_manager_disallowed_takes_precedence_over_tools() {
    let definition = AgentDefinition {
        name: "custom".into(),
        description: "custom".into(),
        tools: vec!["Grep".into(), "Read".into()],
        disallowed_tools: vec!["Grep".into()],
        model: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    };
    let manager = build_filtered_tool_manager(SubAgentKind::Explore, &definition);
    assert!(
        manager.get_tool("read_file").is_some(),
        "Read should be allowed"
    );
    assert!(
        manager.get_tool("grep").is_none(),
        "Grep should be blocked by disallowed_tools"
    );
}

#[test]
fn resolve_kind_definition_plan_sets_plan_mode_required() {
    let def = resolve_kind_definition(SubAgentKind::Plan);
    assert!(def.plan_mode_required);
    assert_eq!(def.name, "plan");
}

#[test]
fn resolve_kind_definition_explore_no_plan_mode() {
    let def = resolve_kind_definition(SubAgentKind::Explore);
    assert!(!def.plan_mode_required);
    assert_eq!(def.name, "explore");
}

#[test]
fn resolve_spawn_agent_definition_resolves_builtin() {
    let def = resolve_spawn_agent_definition("Explore");
    assert_eq!(def.name, "Explore");
}

#[test]
fn resolve_spawn_agent_definition_falls_back_for_unknown() {
    let def = resolve_spawn_agent_definition("unknown-agent");
    assert_eq!(def.name, "unknown-agent");
    assert!(!def.plan_mode_required);
}

#[test]
fn explore_agent_definition_has_default_max_turns_50() {
    let def = resolve_kind_definition(SubAgentKind::Explore);
    assert_eq!(def.max_turns, 50);
}

#[test]
fn plan_agent_definition_has_default_max_turns_30() {
    let def = resolve_kind_definition(SubAgentKind::Plan);
    assert_eq!(def.max_turns, 30);
}

#[test]
fn general_agent_definition_has_unlimited_max_turns() {
    let def = resolve_kind_definition(SubAgentKind::General);
    assert_eq!(def.max_turns, 0);
}
