use super::*;
use crate::tui::runtime::{QueryStopKind, QueryTaskControl};

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
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
        query_control: None,
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
        query_control: Some(QueryTaskControl::new("test-session".into())),
    });

    assert!(request_running_task_cancellation(
        &mut app,
        QueryStopKind::Cancel
    ));

    assert!(token.load(Ordering::SeqCst));
    assert!(
        app.bottom_pane
            .running_task
            .as_ref()
            .and_then(|task| task.query_control.as_ref())
            .and_then(QueryTaskControl::stop_kind)
            .is_some_and(|kind| kind == QueryStopKind::Cancel)
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
