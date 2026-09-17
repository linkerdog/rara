use super::*;

#[tokio::test]
async fn busy_submit_queues_follow_up_message() {
    let temp = tempdir().expect("tempdir");
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

    app.bottom_pane.input = "continue with the follow-up".into();

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            unreachable!()
        }),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = crate::tui::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert_eq!(
        app.queued_follow_up_preview(),
        Some("continue with the follow-up")
    );
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Queued for after the next tool call boundary"))
    );
    assert_eq!(
        app.pending_follow_up_preview(),
        Some("continue with the follow-up")
    );

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn status_overlay_shortcuts_switch_tabs() {
    let temp = tempdir().expect("tempdir");
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

    app.overlay = Some(Overlay::Status(StatusTab::Overview));

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('2')), &app),
        AppEvent::SelectStatusTab(StatusTab::Config)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Right), &app),
        AppEvent::SelectStatusTab(StatusTab::Config)
    ));

    app.overlay = Some(Overlay::Status(StatusTab::Context));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Right), &app),
        AppEvent::SelectStatusTab(StatusTab::Overview)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Left), &app),
        AppEvent::SelectStatusTab(StatusTab::Config)
    ));
}

#[test]
fn context_overlay_scroll_keybindings() {
    let temp = tempdir().expect("tempdir");
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

    app.open_overlay(Overlay::Context);

    // j / Down scroll down → positive delta
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('j')), &app),
        AppEvent::ScrollContext(1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::ScrollContext(1)
    ));
    // k / Up scroll up → negative delta
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('k')), &app),
        AppEvent::ScrollContext(-1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::ScrollContext(-1)
    ));
    // Esc / Enter close
    assert!(matches!(
        map_key_to_event(key(KeyCode::Esc), &app),
        AppEvent::CloseOverlay
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Enter), &app),
        AppEvent::CloseOverlay
    ));
}

#[test]
fn context_scroll_direction_is_top_down() {
    let temp = tempdir().expect("tempdir");
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

    app.open_overlay(Overlay::Context);
    assert_eq!(app.context_scroll, 0);

    // Down / j → scroll away from top, offset increases
    app.scroll_context(1);
    assert_eq!(app.context_scroll, 1);
    app.scroll_context(1);
    assert_eq!(app.context_scroll, 2);

    // Up / k → scroll back toward top, offset decreases
    app.scroll_context(-1);
    assert_eq!(app.context_scroll, 1);
    app.scroll_context(-1);
    assert_eq!(app.context_scroll, 0);

    // Cannot go below 0
    app.scroll_context(-1);
    assert_eq!(app.context_scroll, 0);

    // PageDown / PageUp
    app.scroll_context(5);
    assert_eq!(app.context_scroll, 5);
    app.scroll_context(-5);
    assert_eq!(app.context_scroll, 0);

    // Reopen resets scroll
    app.scroll_context(10);
    assert_eq!(app.context_scroll, 10);
    app.open_overlay(Overlay::Context);
    assert_eq!(app.context_scroll, 0);
}

#[tokio::test]
async fn pending_plan_approval_blocks_plain_submit() {
    let temp = tempdir().expect("tempdir");
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

    app.show_pending_plan_approval(None);
    app.bottom_pane.input = "start implementation".into();

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = crate::tui::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.has_pending_plan_approval());
    assert!(app.bottom_pane.running_task.is_none());
    let notice = app.bottom_pane.notice.as_deref().expect("notice");
    assert!(notice.contains("Use 1 approve"));
    assert!(notice.contains("2 keep planning"));
    assert!(notice.contains("3 reject"));
}

#[test]
fn pending_plan_approval_number_shortcuts_work_in_local_and_ssh() {
    for ssh in [false, true] {
        let _ssh_env = crate::tui::terminal_ui::test_env::set_ssh_session(ssh);
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");
        app.show_pending_plan_approval(None);

        assert_eq!(app.active_pending_option_count(), 3);
        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('1')), &app),
            AppEvent::SelectPendingOption(0)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('2')), &app),
            AppEvent::SelectPendingOption(1)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('3')), &app),
            AppEvent::SelectPendingOption(2)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Down), &app),
            AppEvent::MoveApprovalSelection(1)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Enter), &app),
            AppEvent::SelectPendingOption(0)
        ));
    }
}

#[tokio::test]
async fn submit_numeric_input_handles_pending_shell_approval() {
    let temp = tempdir().expect("tempdir");
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

    add_pending_shell_approval(&mut app);
    app.bottom_pane.input = "4".into();

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = crate::tui::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.bottom_pane.running_task.is_none());
    assert_eq!(app.bottom_pane.input, "");
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Approval is still preparing"))
    );
}

#[tokio::test]
async fn plan_approval_reject_clears_pending_without_starting_task() {
    let temp = tempdir().expect("tempdir");
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

    add_pending_plan_approval(&mut app);

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

    dispatch_event(
        AppEvent::SelectPendingOption(2),
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("reject plan");

    assert!(!app.has_pending_plan_approval());
    assert!(app.bottom_pane.running_task.is_none());
    assert!(agent_slot.is_some());
    assert_eq!(app.agent_execution_mode_label(), "execute");
    assert_eq!(
        app.completed_interaction(InteractionKind::PlanApproval)
            .map(|interaction| interaction.summary.as_str()),
        Some("Rejected. Implementation cancelled.")
    );
}

#[tokio::test]
async fn invalid_plan_approval_selection_keeps_pending_with_notice() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    add_pending_plan_approval(&mut app);

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

    dispatch_event(
        AppEvent::SelectPendingOption(3),
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("reject invalid plan selection");

    assert!(app.has_pending_plan_approval());
    assert!(agent_slot.is_some());
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Invalid plan approval option"))
    );
    assert!(
        app.completed_interaction(InteractionKind::PlanApproval)
            .is_none()
    );
}

#[tokio::test]
async fn empty_submit_keeps_shell_approval_on_card_surface() {
    let temp = tempdir().expect("tempdir");
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

    add_pending_shell_approval(&mut app);

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = crate::tui::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.overlay.is_none());
    assert_eq!(app.approval_picker_idx, 0);
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Left/Right and Enter"))
    );
}

#[tokio::test]
async fn plain_submit_queues_while_shell_approval_is_pending() {
    let temp = tempdir().expect("tempdir");
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

    add_pending_shell_approval(&mut app);
    app.bottom_pane.input = "then review the diff".into();

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = crate::tui::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.bottom_pane.running_task.is_none());
    assert_eq!(app.queued_follow_up_preview(), Some("then review the diff"));
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("pending interaction is answered"))
    );
}

#[tokio::test]
async fn esc_cancels_busy_query_without_overlay() {
    let temp = tempdir().expect("tempdir");
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
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            unreachable!()
        }),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });

    assert!(matches!(
        map_key_to_event(key(KeyCode::Esc), &app),
        AppEvent::CancelRunningTask
    ));

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn busy_submit_allows_quit_command() {
    let temp = tempdir().expect("tempdir");
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

    app.bottom_pane.input = "/quit".into();

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::OAuth,
        receiver,
        handle: tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            unreachable!()
        }),
        started_at: Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        cancellation_requested: false,
    });

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = crate::tui::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(should_quit);

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}
