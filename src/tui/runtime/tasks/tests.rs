use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use rara_memory::memory_handle::MemoryHandle;
use rara_provider_catalog::ModelCatalogProvider;
use rara_tools::planning::{EnterPlanModeTool, ExitPlanModeTool};
use rara_tools::tool::ToolManager;
use secrecy::ExposeSecret;
use serde_json::json;
use tempfile::tempdir;
use tokio::sync::{Mutex, mpsc};

use super::{
    emit_query_heartbeat, finish_running_task_if_ready, forward_optional_lifecycle_event_to_bus,
    forward_task_result_lifecycle, goal_budget_limit_prompt, goal_continuation_prompt,
    merge_rebuilt_agent, model_catalog_connection, request_running_task_cancellation,
    start_oauth_task, start_query_task, try_start_queued_follow_up,
};
use crate::agent::{
    Agent, AgentExecutionMode, BashApprovalMode, Message, PlanStep, PlanStepStatus,
};
use crate::config::{ConfigManager, DEFAULT_CODEX_BASE_URL, DEFAULT_KIMI_BASE_URL, RaraConfig};
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, TokenUsage};
use crate::oauth::OAuthManager;
use crate::prompt::PromptRuntimeConfig;
use crate::runtime_control::{
    ErrorEvent, RuntimeControllerKind, RuntimeEvent, RuntimeProvenance, SessionEvent,
};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::session::SessionManager;
use crate::tui::message_role::MessageRole;
use crate::tui::state::{
    GoalStatus, OAuthLoginMode, RalphGoal, RebuildSuccess, RunningTask, RuntimePhase,
    TaskCompletion, TaskKind, TuiApp,
};
use crate::workspace::WorkspaceMemory;

mod recovery;

struct PlainAnswerBackend;

#[path = "continuity_tests.rs"]
mod continuity_tests;

#[test]
fn model_catalog_connection_uses_target_provider_credentials() {
    let mut config = RaraConfig {
        provider: "codex".to_string(),
        ..Default::default()
    };
    config.set_api_key("sk-codex");
    config.set_base_url(Some(DEFAULT_CODEX_BASE_URL.to_string()));
    config.set_provider_api_key("kimi", "sk-kimi");

    let (api_key, base_url) = model_catalog_connection(&config, ModelCatalogProvider::Kimi);

    assert_eq!(config.provider, "codex");
    assert_eq!(config.api_key(), Some("sk-codex"));
    assert_eq!(
        api_key.as_ref().map(ExposeSecret::expose_secret),
        Some("sk-kimi")
    );
    assert_eq!(base_url, DEFAULT_KIMI_BASE_URL);
}

#[test]
fn lifecycle_helper_publishes_turn_finished_for_success() {
    let bus = Arc::new(RuntimeEventBus::new(8));
    let mut control = bus.subscribe_control();
    let provenance = RuntimeProvenance::local_tui("session-1");

    forward_task_result_lifecycle(&bus, &provenance, &Ok::<_, anyhow::Error>(()));

    let event = control.try_recv().expect("control event");
    assert_eq!(event.provenance.controller, RuntimeControllerKind::LocalTui);
    assert!(matches!(
        event.event,
        RuntimeEvent::Session(SessionEvent::TurnFinished {
            reason: Some(reason)
        }) if reason == "turn complete"
    ));
}

#[test]
fn unrequested_cancellation_text_preserves_maintenance_failure() {
    let bus = Arc::new(RuntimeEventBus::new(8));
    let mut control = bus.subscribe_control();
    let provenance = RuntimeProvenance::local_tui("session-1");

    forward_task_result_lifecycle::<()>(
        &bus,
        &provenance,
        &Err(anyhow::anyhow!("cancelled by user")),
    );

    let event = control.try_recv().expect("control event");
    assert!(matches!(
        event.event,
        RuntimeEvent::Error(ErrorEvent::RuntimeError {
            message,
            recoverable: false,
        }) if message == "cancelled by user"
    ));
}

#[test]
fn lifecycle_helper_publishes_runtime_error_for_failure() {
    let bus = Arc::new(RuntimeEventBus::new(8));
    let mut control = bus.subscribe_control();
    let provenance = RuntimeProvenance::local_tui("session-1");

    forward_task_result_lifecycle::<()>(&bus, &provenance, &Err(anyhow::anyhow!("backend failed")));

    let event = control.try_recv().expect("control event");
    assert!(matches!(
        event.event,
        RuntimeEvent::Error(ErrorEvent::RuntimeError {
            message,
            recoverable: false,
        }) if message == "backend failed"
    ));
}

#[test]
fn optional_lifecycle_helper_publishes_turn_started_when_bus_exists() {
    let bus = Arc::new(RuntimeEventBus::new(8));
    let mut control = bus.subscribe_control();
    let provenance = RuntimeProvenance::local_tui("session-1");

    forward_optional_lifecycle_event_to_bus(
        &Some(bus),
        crate::agent::AgentEvent::AgentStart,
        &provenance,
    );
    forward_optional_lifecycle_event_to_bus(
        &None,
        crate::agent::AgentEvent::AgentStart,
        &provenance,
    );

    let event = control.try_recv().expect("control event");
    assert!(matches!(
        event.event,
        RuntimeEvent::Session(SessionEvent::TurnStarted)
    ));
    assert!(control.try_recv().is_err());
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
    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> anyhow::Result<String> {
        Ok("summary".to_string())
    }
}

fn create_test_agent(temp: &tempfile::TempDir) -> Agent {
    create_test_agent_with_backend(temp, Arc::new(crate::llm::MockLlm))
}

fn create_test_agent_with_backend(temp: &tempfile::TempDir, backend: Arc<dyn LlmBackend>) -> Agent {
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
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager,
        workspace,
    )
}

fn install_completed_query_task(app: &mut TuiApp, agent: Agent, result: anyhow::Result<()>) {
    let (_sender, receiver) = mpsc::unbounded_channel();
    let goal_turn = (!matches!(app.agent_execution_mode, AgentExecutionMode::Plan))
        .then(|| app.goal_handle.begin_turn(app.snapshot.total_input_tokens))
        .flatten();
    let handle = tokio::spawn(async move {
        TaskCompletion::Query {
            agent,
            result,
            goal_turn,
        }
    });
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: None,
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
        query_control: None,
    });
}

fn rebuild_success(temp: &tempfile::TempDir) -> RebuildSuccess {
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    RebuildSuccess {
        agent: create_test_agent(temp),
        warnings: Vec::new(),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
        goal_handle: Arc::new(crate::runtime_goals::GoalStore::default()),
        mcp_tool_cache: crate::mcp_tool_cache::McpToolCache::new(),
        mcp_manager: Arc::new(crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
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

fn install_runtime_services(app: &mut TuiApp) {
    let bus = Arc::new(RuntimeEventBus::new(10));
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
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus),
    ));
}

#[tokio::test]
async fn rebuild_success_keeps_long_warnings_in_transcript() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let warning = "backend bootstrap reported: failed to initialize optional dependency: install failed with status exit status: 1: ERROR: ResolutionImpossible".to_string();
    let mut success = rebuild_success(&temp);
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
            .any(|entry| entry.role == MessageRole::System && entry.message == warning)
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
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    let mut agent_slot = Some(agent);

    try_start_queued_follow_up(&mut app, &mut agent_slot, None);

    assert_eq!(app.queued_follow_up_count(), 0);
    assert!(app.bottom_pane.running_task.is_some());
    assert_eq!(app.active_turn.entries.len(), 1);
    assert_eq!(app.active_turn.entries[0].role, MessageRole::User);
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
    assert_eq!(app.active_turn.entries[0].role, MessageRole::User);
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
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));
    let agent = create_test_agent(&temp);
    app.queue_follow_up_message("continue after cancel");
    let control = super::QueryTaskControl::new(agent.session_id.clone());
    control.request_stop(super::QueryStopKind::Cancel, &AtomicBool::new(false));
    install_completed_query_task(&mut app, agent, Err(anyhow::anyhow!("cancelled by user")));
    app.bottom_pane.running_task.as_mut().unwrap().query_control = Some(control);

    let mut agent_slot = None;
    finish_ready_query_task(&mut app, &mut agent_slot).await;

    assert_eq!(app.queued_follow_up_count(), 0);
    assert!(app.bottom_pane.running_task.is_some());
    assert_eq!(app.active_turn.entries.len(), 1);
    assert_eq!(app.active_turn.entries[0].role, MessageRole::User);
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
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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

mod plan_and_control;
mod query_lifecycle;
