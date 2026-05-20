use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use rara_memory::vectordb::VectorDB;
use rara_tools::planning::{EnterPlanModeTool, ExitPlanModeTool};
use rara_tools::tool::ToolManager;
use serde_json::json;
use tempfile::tempdir;
use tokio::sync::{Mutex, mpsc};

use super::{
    emit_query_heartbeat, finish_running_task_if_ready, goal_budget_limit_prompt,
    goal_continuation_prompt, merge_rebuilt_agent, request_running_task_cancellation,
    start_oauth_task, start_query_task, try_start_queued_follow_up,
};
use crate::agent::{
    Agent, AgentExecutionMode, BashApprovalMode, Message, PlanStep, PlanStepStatus,
};
use crate::config::ConfigManager;
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, TokenUsage};
use crate::local_model_server::{LocalModelServerState, LocalModelServerStatus};
use crate::oauth::OAuthManager;
use crate::prompt::PromptRuntimeConfig;
use crate::session::SessionManager;
use crate::tui::state::{
    OAuthLoginMode, RalphGoal, RebuildSuccess, RunningTask, RuntimePhase, TaskCompletion, TaskKind,
    TuiApp,
};
use crate::workspace::WorkspaceMemory;

struct PlainAnswerBackend;

#[test]
fn goal_continuation_prompt_contains_budget_and_completion_audit() {
    let mut goal = RalphGoal::new("ship Codex 0.130 goal parity".to_string(), Some(10_000));
    goal.tokens_used = 2_500;
    goal.turns_completed = 2;

    let prompt = goal_continuation_prompt(&goal);

    assert!(prompt.contains("<untrusted_objective>"));
    assert!(prompt.contains("ship Codex 0.130 goal parity"));
    assert!(prompt.contains("Tokens used: 2500"));
    assert!(prompt.contains("Token budget: 10000"));
    assert!(prompt.contains("Tokens remaining: 7500"));
    assert!(prompt.contains("call update_goal with status \"complete\""));
}

#[test]
fn goal_budget_limit_prompt_asks_for_wrap_up_without_new_work() {
    let mut goal = RalphGoal::new("finish the migration".to_string(), Some(100));
    goal.tokens_used = 100;

    let prompt = goal_budget_limit_prompt(&goal);

    assert!(prompt.contains("has reached its token budget"));
    assert!(prompt.contains("Do not start new substantive work"));
    assert!(prompt.contains("finish the migration"));
    assert!(prompt.contains("Token budget: 100"));
}

#[async_trait::async_trait]
impl LlmBackend for PlainAnswerBackend {
    async fn ask(
        &self,
        _messages: &[crate::agent::Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Planning analysis only.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 8])
    }

    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> anyhow::Result<String> {
        Ok("summary".to_string())
    }
}

struct AgentDrivenPlanBackend {
    calls: Mutex<usize>,
}

#[async_trait::async_trait]
impl LlmBackend for AgentDrivenPlanBackend {
    async fn ask(
        &self,
        _messages: &[crate::agent::Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        let mut calls = self.calls.lock().await;
        *calls += 1;
        if *calls == 1 {
            return Ok(LlmResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "enter-plan".to_string(),
                    name: "enter_plan_mode".to_string(),
                    input: json!({}),
                }],
                stop_reason: Some("tool_use".to_string()),
                usage: Some(TokenUsage::default()),
            });
        }
        let text = match *calls {
            2 => {
                "<proposed_plan>\n- [pending] Inspect the TUI state machine\n- [pending] Update focused tests\n</proposed_plan>"
            }
            _ => "Implemented the auto-approved plan and reviewed the changes.",
        };
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 8])
    }

    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> anyhow::Result<String> {
        Ok("summary".to_string())
    }
}

struct ExitPlanModeBackend;

#[async_trait::async_trait]
impl LlmBackend for ExitPlanModeBackend {
    async fn ask(
        &self,
        _messages: &[crate::agent::Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Fix plan exit handling\n</proposed_plan>"
                        .to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; 8])
    }

    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> anyhow::Result<String> {
        Ok("summary".to_string())
    }
}

fn create_test_agent(temp: &tempfile::TempDir) -> Agent {
    let workspace_root = temp.path().join("workspace");
    let rara_dir = workspace_root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let workspace = Arc::new(WorkspaceMemory::from_paths(
        workspace_root.clone(),
        rara_dir.clone(),
    ));
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    Agent::new(
        ToolManager::new(),
        Arc::new(crate::llm::MockLlm),
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    )
}

fn install_completed_query_task(app: &mut TuiApp, agent: Agent, result: anyhow::Result<()>) {
    let (_sender, receiver) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move { TaskCompletion::Query { agent, result } });
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

fn install_completed_rebuild_task(app: &mut TuiApp, success: RebuildSuccess) {
    let (_sender, receiver) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        TaskCompletion::Rebuild {
            result: Ok(success),
        }
    });
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Rebuild,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

fn rebuild_success(
    temp: &tempfile::TempDir,
    local_model_server: LocalModelServerStatus,
) -> RebuildSuccess {
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    RebuildSuccess {
        agent: create_test_agent(temp),
        warnings: Vec::new(),
        local_model_server,
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
        goal_handle: Arc::new(std::sync::RwLock::new(None)),
        mcp_tool_cache: crate::mcp_tool_cache::McpToolCache::new(),
        mcp_manager: Arc::new(crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        )),
        prompt_source_registry: Arc::new(crate::protocol_sources::PromptSourceRegistry::new(
            bus.clone(),
        )),
        skill_source_registry: Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
            bus.clone(),
        )),
        hook_registry: Arc::new(crate::hook_registry::HookRegistry::new(bus.clone())),
        hook_runtime: Arc::new(crate::hook_runtime::HookRuntime::new(bus.clone())),
        memory_handler: Arc::new(crate::protocol_sources::MemoryControlHandler::new(bus)),
        lsp_manager: Arc::new(crate::lsp_manager::LspManager::new(
            temp.path().to_path_buf(),
        )),
    }
}

async fn finish_ready_query_task(app: &mut TuiApp, agent_slot: &mut Option<Agent>) {
    for _ in 0..20 {
        finish_running_task_if_ready(app, agent_slot)
            .await
            .expect("finish task");
        if app
            .bottom_pane
            .running_task
            .as_ref()
            .is_some_and(|task| matches!(task.kind, TaskKind::Query))
            && !app.has_queued_follow_up_messages()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn rebuild_success_refreshes_local_model_server_status() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.local_model_server = LocalModelServerStatus {
        state: LocalModelServerState::SetupRequired,
        backend: "mlx_qwen3".to_string(),
        model: "mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ".to_string(),
        detail: "stale startup status".to_string(),
        server_path: None,
        endpoint: None,
    };
    let ready_status = LocalModelServerStatus {
        state: LocalModelServerState::Ready,
        backend: "mlx_qwen3".to_string(),
        model: "mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ".to_string(),
        detail: "started model server and prepared model".to_string(),
        server_path: Some(temp.path().join("rara_model_server.py")),
        endpoint: Some("http://127.0.0.1:18181".to_string()),
    };
    install_completed_rebuild_task(&mut app, rebuild_success(&temp, ready_status.clone()));

    let mut agent_slot = Some(create_test_agent(&temp));
    for _ in 0..20 {
        finish_running_task_if_ready(&mut app, &mut agent_slot)
            .await
            .expect("finish rebuild task");
        if app.bottom_pane.running_task.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(app.local_model_server, ready_status);
    assert_eq!(app.runtime_phase, RuntimePhase::BackendReady);
}

#[tokio::test]
async fn rebuild_success_keeps_long_warnings_in_transcript() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let ready_status = LocalModelServerStatus {
        state: LocalModelServerState::Ready,
        backend: "mlx_qwen3".to_string(),
        model: "mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ".to_string(),
        detail: "started model server and prepared model".to_string(),
        server_path: Some(temp.path().join("rara_model_server.py")),
        endpoint: Some("http://127.0.0.1:18181".to_string()),
    };
    let warning = "local embedding backend bootstrap reported: failed to install model server dependencies: install model server dependencies failed with status exit status: 1: ERROR: ResolutionImpossible".to_string();
    let mut success = rebuild_success(&temp, ready_status);
    success.warnings = vec![warning.clone()];
    install_completed_rebuild_task(&mut app, success);

    let mut agent_slot = Some(create_test_agent(&temp));
    for _ in 0..20 {
        finish_running_task_if_ready(&mut app, &mut agent_slot)
            .await
            .expect("finish rebuild task");
        if app.bottom_pane.running_task.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        app.bottom_pane.notice.as_deref(),
        Some("Startup warning added to transcript.")
    );
    assert!(
        app.committed_turns
            .iter()
            .flat_map(|turn| turn.entries.iter())
            .any(|entry| entry.role == "System" && entry.message == warning)
    );
}

#[test]
fn browser_oauth_is_rejected_before_task_start_in_ssh() {
    let temp = tempdir().unwrap();
    let _ssh_env = crate::tui::terminal_ui::test_env::set_ssh_session(true);

    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager"),
    );

    start_oauth_task(&mut app, oauth_manager, OAuthLoginMode::Browser);

    assert!(app.bottom_pane.running_task.is_none());
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Browser login is unavailable"))
    );
}

#[test]
fn merge_rebuilt_agent_preserves_session_and_turn_state() {
    let temp = tempdir().unwrap();
    let workspace_root = temp.path().join("workspace");
    let rara_dir = workspace_root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let workspace = Arc::new(WorkspaceMemory::from_paths(
        workspace_root.clone(),
        rara_dir.clone(),
    ));
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    let backend = Arc::new(crate::llm::MockLlm);

    let mut previous = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager.clone(),
        workspace.clone(),
    );
    previous.session_id = "session-keep".to_string();
    previous.history.push(Message {
        role: "user".into(),
        content: json!([{"type":"text","text":"keep history"}]),
    });
    previous.total_input_tokens = 123;
    previous.total_output_tokens = 45;
    previous.total_cache_hit_tokens = 90;
    previous.total_cache_miss_tokens = 10;
    previous.execution_mode = AgentExecutionMode::Plan;
    previous.bash_approval_mode = BashApprovalMode::Suggestion;
    previous.set_full_access_mode(true);
    previous.approved_bash_prefixes = vec!["git push".to_string()];
    previous.current_plan = vec![PlanStep {
        step: "Keep session continuity".into(),
        status: PlanStepStatus::InProgress,
    }];
    previous.plan_explanation = Some("Do not reset the session during model switch.".into());
    previous.compact_state.estimated_history_tokens = 1_200;
    previous.compact_state.context_window_tokens = Some(8_192);
    previous.compact_state.compact_threshold_tokens = 7_000;
    previous.compact_state.reserved_output_tokens = 1_024;
    previous.compact_state.compaction_count = 2;
    previous.compact_state.last_compaction_before_tokens = Some(5_000);
    previous.compact_state.last_compaction_after_tokens = Some(2_100);
    previous.compact_state.last_compaction_recent_files = vec!["src/main.rs".into()];
    previous.compact_state.last_compaction_boundary = Some(crate::agent::CompactBoundaryMetadata {
        version: 1,
        before_tokens: 5_000,
        recent_file_count: 1,
    });
    previous.set_prompt_config(PromptRuntimeConfig {
        append_system_prompt: Some("keep appendix".to_string()),
        warnings: vec!["missing custom prompt".to_string()],
        ..PromptRuntimeConfig::default()
    });

    let mut rebuilt = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new(
            &rara_dir.join("other-lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    rebuilt.compact_state.context_window_tokens = Some(200_000);
    rebuilt.compact_state.compact_threshold_tokens = 180_000;
    rebuilt.compact_state.reserved_output_tokens = 8_192;

    let merged = merge_rebuilt_agent(rebuilt, previous);

    assert_eq!(merged.session_id, "session-keep");
    assert_eq!(merged.history.len(), 1);
    assert_eq!(merged.total_input_tokens, 123);
    assert_eq!(merged.total_output_tokens, 45);
    assert_eq!(merged.total_cache_hit_tokens, 90);
    assert_eq!(merged.total_cache_miss_tokens, 10);
    assert_eq!(merged.execution_mode, AgentExecutionMode::Plan);
    assert_eq!(merged.bash_approval_mode, BashApprovalMode::Suggestion);
    assert!(merged.full_access_mode);
    assert_eq!(merged.approved_bash_prefixes, vec!["git push".to_string()]);
    assert_eq!(merged.current_plan.len(), 1);
    assert_eq!(merged.compact_state.estimated_history_tokens, 1_200);
    assert_eq!(merged.compact_state.compaction_count, 2);
    assert_eq!(
        merged.compact_state.last_compaction_before_tokens,
        Some(5_000)
    );
    assert_eq!(
        merged.compact_state.last_compaction_after_tokens,
        Some(2_100)
    );
    assert_eq!(
        merged.compact_state.last_compaction_recent_files,
        vec!["src/main.rs".to_string()]
    );
    assert_eq!(merged.compact_state.context_window_tokens, Some(200_000));
    assert_eq!(merged.compact_state.compact_threshold_tokens, 180_000);
    assert_eq!(merged.compact_state.reserved_output_tokens, 8_192);
    assert_eq!(
        merged.prompt_config().append_system_prompt.as_deref(),
        Some("keep appendix")
    );
    assert_eq!(
        merged.prompt_config().warnings,
        vec!["missing custom prompt".to_string()]
    );
}

#[tokio::test]
async fn queued_follow_ups_start_as_one_multiline_turn() {
    let temp = tempdir().unwrap();
    let workspace_root = temp.path().join("workspace");
    let rara_dir = workspace_root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    app.queue_follow_up_message("first line");
    app.queue_follow_up_message("second line");

    let workspace = Arc::new(WorkspaceMemory::from_paths(
        workspace_root.clone(),
        rara_dir.clone(),
    ));
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    let agent = Agent::new(
        ToolManager::new(),
        Arc::new(crate::llm::MockLlm),
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    let mut agent_slot = Some(agent);

    try_start_queued_follow_up(&mut app, &mut agent_slot);

    assert_eq!(app.queued_follow_up_count(), 0);
    assert!(app.bottom_pane.running_task.is_some());
    assert_eq!(app.active_turn.entries.len(), 1);
    assert_eq!(app.active_turn.entries[0].role, "You");
    assert_eq!(
        app.active_turn.entries[0].message,
        "first line\n\nsecond line"
    );

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn queued_follow_up_starts_after_query_failure() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    let agent = create_test_agent(&temp);
    app.queue_follow_up_message("inspect the failure");
    install_completed_query_task(&mut app, agent, Err(anyhow::anyhow!("backend failed")));

    let mut agent_slot = None;
    finish_ready_query_task(&mut app, &mut agent_slot).await;

    assert_eq!(app.queued_follow_up_count(), 0);
    assert!(app.bottom_pane.running_task.is_some());
    assert_eq!(app.active_turn.entries.len(), 1);
    assert_eq!(app.active_turn.entries[0].role, "You");
    assert_eq!(app.active_turn.entries[0].message, "inspect the failure");

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn queued_follow_up_starts_after_query_cancellation() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    let agent = create_test_agent(&temp);
    app.queue_follow_up_message("continue after cancel");
    install_completed_query_task(&mut app, agent, Err(anyhow::anyhow!("cancelled by user")));

    let mut agent_slot = None;
    finish_ready_query_task(&mut app, &mut agent_slot).await;

    assert_eq!(app.queued_follow_up_count(), 0);
    assert!(app.bottom_pane.running_task.is_some());
    assert_eq!(app.active_turn.entries.len(), 1);
    assert_eq!(app.active_turn.entries[0].role, "You");
    assert_eq!(app.active_turn.entries[0].message, "continue after cancel");

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn plan_turn_completion_keeps_plan_mode_after_plain_answer() {
    let temp = tempdir().unwrap();
    let workspace_root = temp.path().join("workspace");
    let rara_dir = workspace_root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    app.set_agent_execution_mode(AgentExecutionMode::Plan);

    let workspace = Arc::new(WorkspaceMemory::from_paths(
        workspace_root.clone(),
        rara_dir.clone(),
    ));
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(PlainAnswerBackend),
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    start_query_task(&mut app, "inspect only".to_string(), agent);
    let mut agent_slot = None;
    for _ in 0..20 {
        finish_running_task_if_ready(&mut app, &mut agent_slot)
            .await
            .expect("finish task");
        if app.bottom_pane.running_task.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(app.bottom_pane.running_task.is_none());
    assert_eq!(app.agent_execution_mode, AgentExecutionMode::Plan);
    assert!(!app.has_pending_plan_approval());
    assert_eq!(
        agent_slot
            .as_ref()
            .expect("agent should return")
            .execution_mode,
        AgentExecutionMode::Plan
    );
}

#[tokio::test]
async fn agent_driven_plan_mode_auto_approves_and_resumes_execution() {
    let temp = tempdir().unwrap();
    let workspace_root = temp.path().join("workspace");
    let rara_dir = workspace_root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    app.set_agent_execution_mode(AgentExecutionMode::Execute);

    let workspace = Arc::new(WorkspaceMemory::from_paths(
        workspace_root.clone(),
        rara_dir.clone(),
    ));
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(EnterPlanModeTool));
    let mut agent = Agent::new(
        tool_manager,
        Arc::new(AgentDrivenPlanBackend {
            calls: Mutex::new(0),
        }),
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Execute);

    start_query_task(&mut app, "inspect and plan".to_string(), agent);
    let mut agent_slot = None;
    for _ in 0..20 {
        finish_running_task_if_ready(&mut app, &mut agent_slot)
            .await
            .expect("finish task");
        if app.bottom_pane.running_task.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(app.bottom_pane.running_task.is_none());
    assert_eq!(app.agent_execution_mode, AgentExecutionMode::Execute);
    assert!(!app.has_pending_plan_approval());
    let agent = agent_slot.as_ref().expect("agent should return");
    assert_eq!(agent.execution_mode, AgentExecutionMode::Execute);
    assert_eq!(agent.current_plan.len(), 2);
    assert!(
        agent
            .history
            .last()
            .is_some_and(|message| message.content.to_string().contains("reviewed the changes"))
    );
}

#[tokio::test]
async fn exit_plan_mode_stops_for_plan_approval() {
    let temp = tempdir().unwrap();
    let workspace_root = temp.path().join("workspace");
    let rara_dir = workspace_root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    app.set_agent_execution_mode(AgentExecutionMode::Plan);

    let workspace = Arc::new(WorkspaceMemory::from_paths(
        workspace_root.clone(),
        rara_dir.clone(),
    ));
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let mut agent = Agent::new(
        tool_manager,
        Arc::new(ExitPlanModeBackend),
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    start_query_task(&mut app, "prepare a plan".to_string(), agent);
    let mut agent_slot = None;
    for _ in 0..20 {
        finish_running_task_if_ready(&mut app, &mut agent_slot)
            .await
            .expect("finish task");
        if app.bottom_pane.running_task.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(app.bottom_pane.running_task.is_none());
    assert_eq!(app.agent_execution_mode, AgentExecutionMode::Plan);
    assert!(app.has_pending_plan_approval());
    let agent = agent_slot.as_ref().expect("agent should return");
    assert!(agent.has_pending_plan_exit_approval());
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
}

#[tokio::test]
async fn query_heartbeat_preserves_running_tool_phase() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    let (_sender, receiver) = mpsc::unbounded_channel();
    let handle = tokio::spawn(std::future::pending::<TaskCompletion>());
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now() - Duration::from_secs(3),
        next_heartbeat_after_secs: 0,
        cancellation_token: None,
        cancellation_requested: false,
    });
    app.set_runtime_phase(
        RuntimePhase::RunningTool,
        Some("streaming bash output".into()),
    );

    emit_query_heartbeat(&mut app);

    assert_eq!(app.runtime_phase, RuntimePhase::RunningTool);
    assert_eq!(
        app.runtime_phase_detail.as_deref(),
        Some("streaming bash output · 3s elapsed")
    );
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn query_cancellation_sets_running_task_token() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    let (_sender, receiver) = mpsc::unbounded_channel();
    let token = Arc::new(AtomicBool::new(false));
    let handle = tokio::spawn(std::future::pending::<TaskCompletion>());
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: Some(token.clone()),
        cancellation_requested: false,
    });

    request_running_task_cancellation(&mut app);

    assert!(token.load(Ordering::SeqCst));
    assert!(
        app.bottom_pane
            .running_task
            .as_ref()
            .is_some_and(|task| task.cancellation_requested)
    );
    assert_eq!(app.runtime_phase, RuntimePhase::ProcessingResponse);
    assert_eq!(
        app.runtime_phase_detail.as_deref(),
        Some("cancelling query")
    );

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}
