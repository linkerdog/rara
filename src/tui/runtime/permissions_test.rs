use std::sync::{Arc, atomic::Ordering};
use std::time::Instant;

use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;
use tokio::sync::mpsc;

use super::{apply_permission_mode, request_permission_mode};
use crate::agent::{Agent, AgentExecutionMode, BashApprovalMode};
use crate::config::{ConfigManager, McpRegistry};
use crate::llm::MockLlm;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tui::state::{PermissionMode, RunningTask, TaskCompletion, TaskKind, TuiApp};

fn fixture() -> (tempfile::TempDir, TuiApp, Agent) {
    let dir = tempfile::tempdir().expect("tempdir");
    let rara_dir = dir.path().join(".rara");
    for child in ["rollouts", "sessions", "tool-results"] {
        std::fs::create_dir_all(rara_dir.join(child)).expect("runtime directory");
    }
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    let agent = Agent::new(
        ToolManager::new(),
        Arc::new(MockLlm),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        Arc::new(crate::session::SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        }),
        Arc::new(crate::workspace::WorkspaceMemory::from_paths(
            dir.path().to_path_buf(),
            rara_dir,
        )),
    );
    let mut slot = Some(agent);
    apply_permission_mode(&mut app, &mut slot, PermissionMode::AcceptEdits);
    let bus = Arc::new(RuntimeEventBus::new(16));
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
            Arc::new(McpRegistry::empty()),
            bus.clone(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus),
    ));
    (dir, app, slot.unwrap())
}

fn mark_busy(app: &mut TuiApp) {
    let (_, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(async {
            TaskCompletion::ModelCatalog {
                provider: rara_provider_catalog::ModelCatalogProvider::DeepSeek,
                result: Ok(vec![]),
            }
        }),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

async fn complete(app: &mut TuiApp, agent: Agent, result: anyhow::Result<()>) -> Option<Agent> {
    let mut slot = None;
    crate::tui::runtime::tasks::finish_running_task_if_ready_from_runtime_port(
        app,
        &mut slot,
        Some(Ok(TaskCompletion::Query { agent, result })),
        None,
    )
    .await
    .expect("completion");
    slot
}

#[tokio::test]
async fn runtime_defers_permissions_until_query_completion() {
    let (_dir, mut app, agent) = fixture();
    mark_busy(&mut app);
    request_permission_mode(&mut app, &mut None, PermissionMode::FullAccess);
    assert_eq!(
        app.pending_permission_mode,
        Some(PermissionMode::FullAccess)
    );
    assert_eq!(app.permission_mode_label(), "accept-edits");
    assert!(!app.sandbox_network_access.load(Ordering::Relaxed));
    assert!(!agent.full_access_mode);
    let agent = complete(&mut app, agent, Ok(()))
        .await
        .expect("returned agent");
    assert!(app.pending_permission_mode.is_none());
    assert_eq!(app.permission_mode_label(), "full-access");
    assert!(app.sandbox_network_access.load(Ordering::Relaxed));
    assert!(agent.full_access_mode);
    assert_eq!(agent.bash_approval_mode, BashApprovalMode::Always);
}

#[tokio::test]
async fn latest_request_wins_and_current_preset_cancels_pending_change() {
    let (_dir, mut app, agent) = fixture();
    mark_busy(&mut app);
    request_permission_mode(&mut app, &mut None, PermissionMode::FullAccess);
    request_permission_mode(&mut app, &mut None, PermissionMode::ReadOnly);
    assert_eq!(app.pending_permission_mode, Some(PermissionMode::ReadOnly));
    request_permission_mode(&mut app, &mut None, PermissionMode::AcceptEdits);
    assert!(app.pending_permission_mode.is_none());
    request_permission_mode(&mut app, &mut None, PermissionMode::ReadOnly);
    let agent = complete(&mut app, agent, Ok(())).await.unwrap();
    assert_eq!(app.permission_mode_label(), "read-only");
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(!agent.full_access_mode);
}

#[tokio::test]
async fn cancellation_applies_pending_permissions_before_queued_input() {
    let (_dir, mut app, agent) = fixture();
    mark_busy(&mut app);
    app.queue_follow_up_message("Inspect the files.");
    request_permission_mode(&mut app, &mut None, PermissionMode::ReadOnly);
    assert!(
        complete(&mut app, agent, Err(anyhow::anyhow!("cancelled by user")))
            .await
            .is_none()
    );
    assert!(app.pending_permission_mode.is_none());
    assert_eq!(app.permission_mode_label(), "read-only");
    let task = app
        .bottom_pane
        .running_task
        .take()
        .expect("queued input started");
    match task.handle.await.expect("queued task") {
        TaskCompletion::Query { agent, .. } => {
            assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
            assert_eq!(agent.bash_approval_mode, BashApprovalMode::Suggestion);
            assert!(!agent.full_access_mode);
        }
        _ => panic!("expected query completion"),
    }
}

#[tokio::test]
async fn idle_permission_change_preserves_pending_plan_decision() {
    let (_dir, mut app, agent) = fixture();
    app.show_pending_plan_approval(Some("exit-plan"));
    let pending = app.snapshot.pending_interactions.clone();
    let mut slot = Some(agent);
    request_permission_mode(&mut app, &mut slot, PermissionMode::FullAccess);
    assert_eq!(app.snapshot.pending_interactions.len(), pending.len());
    assert_eq!(
        app.snapshot.pending_interactions[0].source,
        pending[0].source
    );
    assert_eq!(app.snapshot.pending_interactions[0].title, pending[0].title);
    assert!(slot.unwrap().full_access_mode);
}

#[tokio::test]
async fn resume_picker_retains_explicit_full_access_and_pending_plan() {
    use crate::tui::runtime_port::RuntimeCommand;
    use crate::tui::state::{ListPickerKind, Overlay, RuntimeSnapshot};
    use crate::tui::testing::FakeRuntimeClient;

    let (dir, mut app, mut agent) = fixture();
    agent.session_id = "saved-plan".into();
    agent.set_execution_mode(AgentExecutionMode::Plan);
    agent.history.push(crate::agent::Message {
        role: "user".into(),
        content: serde_json::json!([{"type": "text", "text": "Prepare a plan."}]),
    });
    agent
        .session_manager
        .save_session(&agent.session_id, &agent.history)
        .unwrap();
    let state_db = Arc::new(
        rara_state::state_db::StateDb::new_for_root_dir(agent.workspace.rara_dir.clone()).unwrap(),
    );
    app.attach_state_db(state_db);
    app.apply_runtime_snapshot(
        &agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&agent, 0),
    );
    app.show_pending_plan_approval(Some("exit-plan-restore"));
    agent.session_id = "fresh-session".into();
    app.snapshot.session_id = agent.session_id.clone();
    app.clear_pending_plan_approval();
    let mut slot = Some(agent);
    request_permission_mode(&mut app, &mut slot, PermissionMode::FullAccess);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
    assert_eq!(
        crate::tui::list_picker::selected_resumable_thread_id(&app).as_deref(),
        Some("saved-plan")
    );
    let oauth =
        Arc::new(crate::oauth::OAuthManager::new_for_config_dir(dir.path().join("oauth")).unwrap());
    let port = FakeRuntimeClient::new(RuntimeSnapshot::default());
    crate::tui::event_dispatch::dispatch_event_with_runtime(
        crate::tui::app_event::AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut slot,
        &oauth,
        &port,
    )
    .await
    .unwrap();
    assert_eq!(
        port.commands(),
        vec![RuntimeCommand::SetPermissionMode(
            PermissionMode::FullAccess
        )]
    );
    request_permission_mode(&mut app, &mut slot, PermissionMode::FullAccess);
    assert_eq!(app.permission_mode_label(), "full-access");
    assert!(app.has_pending_plan_approval());
    let agent = slot.unwrap();
    assert_eq!(agent.session_id, "saved-plan");
    assert_eq!(agent.execution_mode, AgentExecutionMode::Execute);
    assert_eq!(agent.bash_approval_mode, BashApprovalMode::Always);
    assert!(agent.full_access_mode);
    assert!(agent.has_pending_plan_exit_approval());
}

struct PlanBackend;

#[async_trait::async_trait]
impl crate::llm::LlmBackend for PlanBackend {
    async fn ask(
        &self,
        _messages: &[crate::agent::Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        Ok(crate::llm::LlmResponse {
            content: vec![crate::llm::ContentBlock::Text { text: "<proposed_plan>\n- [pending] Inspect the files\n- [pending] Update focused checks\n</proposed_plan>".into() }],
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }

    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> anyhow::Result<String> {
        Ok("Plan summary".into())
    }
}

#[tokio::test]
async fn permission_change_at_automatic_plan_boundary_keeps_explicit_decision() {
    let (_dir, mut app, mut agent) = fixture();
    agent.llm_backend = Arc::new(PlanBackend);
    agent.set_execution_mode(AgentExecutionMode::Plan);
    agent
        .query_with_mode_and_events(
            "Prepare the implementation plan.".into(),
            crate::agent::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("plan query");
    assert!(agent.last_query_produced_plan());
    assert!(matches!(
        crate::runtime_client::RuntimeClient::plan_continuation(&agent, false),
        crate::runtime_client::PlanContinuation::AutomaticImplementation
    ));
    mark_busy(&mut app);
    request_permission_mode(&mut app, &mut None, PermissionMode::ReadOnly);
    let agent = complete(&mut app, agent, Ok(()))
        .await
        .expect("agent remains idle");
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert_eq!(app.permission_mode_label(), "read-only");
    assert!(app.has_pending_plan_approval());
    assert!(app.bottom_pane.running_task.is_none());
}

#[tokio::test]
async fn compaction_completion_applies_network_restriction() {
    let (_dir, mut app, agent) = fixture();
    let mut slot = Some(agent);
    request_permission_mode(&mut app, &mut slot, PermissionMode::FullAccess);
    mark_busy(&mut app);
    request_permission_mode(&mut app, &mut slot, PermissionMode::ReadOnly);
    assert!(app.sandbox_network_access.load(Ordering::Relaxed));
    let agent = slot.take().unwrap();
    crate::tui::runtime::tasks::finish_running_task_if_ready_from_runtime_port(
        &mut app,
        &mut slot,
        Some(Ok(TaskCompletion::Compact {
            agent,
            result: Ok(false),
        })),
        None,
    )
    .await
    .expect("compaction completion");
    assert_eq!(app.permission_mode_label(), "read-only");
    assert!(!app.sandbox_network_access.load(Ordering::Relaxed));
    assert!(!slot.unwrap().full_access_mode);
}

#[tokio::test]
async fn lost_agent_rejects_pending_permission_change() {
    let (_dir, mut app, _agent) = fixture();
    mark_busy(&mut app);
    request_permission_mode(&mut app, &mut None, PermissionMode::FullAccess);
    let error = tokio::spawn(async { panic!("scripted task panic") })
        .await
        .expect_err("task failure");
    let result = crate::tui::runtime::tasks::finish_running_task_if_ready_from_runtime_port(
        &mut app,
        &mut None,
        Some(Err(error)),
        None,
    )
    .await;
    assert!(result.is_err());
    assert!(app.pending_permission_mode.is_none());
    assert_eq!(app.permission_mode_label(), "accept-edits");
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .unwrap()
            .contains("Permissions not applied")
    );
}
