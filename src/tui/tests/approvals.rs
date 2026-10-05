use super::*;

#[test]
fn pending_shell_approval_number_shortcuts_work_in_local_and_ssh() {
    for ssh in [false, true] {
        let _ssh_env = crate::tui::terminal_ui::test_env::set_ssh_session(ssh);
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
        app.skill_source_registry = Some(Arc::new(
            crate::protocol_sources::SkillSourceRegistry::new(bus.clone()),
        ));
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

        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('1')), &app),
            AppEvent::SelectPendingOption(0)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('4')), &app),
            AppEvent::SelectPendingOption(3)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::F(2)), &app),
            AppEvent::SelectPendingOption(1)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::F(4)), &app),
            AppEvent::SelectPendingOption(3)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Down), &app),
            AppEvent::MoveApprovalSelection(1)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Right), &app),
            AppEvent::MoveApprovalSelection(1)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('h')), &app),
            AppEvent::MoveApprovalSelection(-1)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('k')), &app),
            AppEvent::MoveApprovalSelection(-1)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Esc), &app),
            AppEvent::SelectPendingOption(3)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Enter), &app),
            AppEvent::SelectPendingOption(0)
        ));
    }
}

#[test]
fn pending_shell_approval_preserves_modifier_shortcuts() {
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

    assert!(matches!(
        map_key_to_event(shifted_key(KeyCode::Enter), &app),
        AppEvent::InsertNewline
    ));
    assert!(matches!(
        map_key_to_event(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            &app
        ),
        AppEvent::InsertNewline
    ));
    app.bottom_pane.input = "draft follow-up".into();
    assert!(matches!(
        map_key_to_event(key(KeyCode::Enter), &app),
        AppEvent::SubmitComposer
    ));
}

#[tokio::test]
async fn pending_shell_approval_card_selection_clamps_with_navigation() {
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

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    for _ in 0..6 {
        dispatch_event(
            AppEvent::MoveApprovalSelection(1),
            &mut app,
            &mut agent_slot,
            &oauth_manager,
        )
        .await
        .expect("move selection down");
    }
    assert_eq!(app.approval_picker_idx, 3);

    for _ in 0..6 {
        dispatch_event(
            AppEvent::MoveApprovalSelection(-1),
            &mut app,
            &mut agent_slot,
            &oauth_manager,
        )
        .await
        .expect("move selection up");
    }
    assert_eq!(app.approval_picker_idx, 0);
}

#[test]
fn pending_shell_approval_does_not_render_as_request_input() {
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

    assert_eq!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(crate::tui::state::ActivePendingInteractionKind::ShellApproval)
    );
    assert_eq!(app.active_pending_option_count(), 4);
}

#[tokio::test]
async fn full_access_permission_picker_preserves_pending_shell_approval_in_local_and_ssh() {
    for ssh in [false, true] {
        let _ssh_env = crate::tui::terminal_ui::test_env::set_ssh_session(ssh);
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
        app.skill_source_registry = Some(Arc::new(
            crate::protocol_sources::SkillSourceRegistry::new(bus.clone()),
        ));
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
        app.open_overlay(Overlay::PermissionPicker);

        let oauth_manager = Arc::new(
            crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
                .expect("oauth manager"),
        );
        let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

        dispatch_event(
            AppEvent::SetPermissionSelection(3),
            &mut app,
            &mut agent_slot,
            &oauth_manager,
        )
        .await
        .expect("select full access");
        dispatch_event(
            AppEvent::ApplyOverlaySelection,
            &mut app,
            &mut agent_slot,
            &oauth_manager,
        )
        .await
        .expect("apply full access");

        assert_eq!(app.permission_mode, PermissionMode::FullAccess);
        assert!(app.pending_command_approval().is_some());
        assert!(agent_slot.is_some());
        assert!(app.bottom_pane.running_task.is_none());
    }
}

#[tokio::test]
async fn session_shell_approval_does_not_promote_full_access_or_network_access() {
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
    let network_access_before = app
        .sandbox_network_access
        .load(std::sync::atomic::Ordering::Relaxed);

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
    .expect("approve for session");

    assert_eq!(app.permission_mode, PermissionMode::Custom);
    assert_eq!(app.bash_approval_mode_label(), "always");
    assert_eq!(
        app.sandbox_network_access
            .load(std::sync::atomic::Ordering::Relaxed),
        network_access_before
    );
    assert!(app.pending_command_approval().is_none());
    assert!(agent_slot.is_none());
    assert!(app.bottom_pane.running_task.is_some());
    abort_running_task(&mut app);
}

#[tokio::test]
async fn full_access_mode_keeps_pending_shell_approval_until_an_explicit_choice() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    add_pending_shell_approval(&mut app);
    app.permission_mode = PermissionMode::FullAccess;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

    dispatch_event(
        AppEvent::SubmitComposer,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("dispatch pending approval");

    assert!(app.pending_command_approval().is_some());
    assert!(agent_slot.is_some());
    assert!(app.bottom_pane.running_task.is_none());
}

#[tokio::test]
async fn full_access_mode_does_not_resume_shell_approval_behind_active_plan_approval() {
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
    add_pending_plan_approval(&mut app);
    app.permission_mode = PermissionMode::FullAccess;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

    dispatch_event(
        AppEvent::SubmitComposer,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("dispatch submit");

    assert_eq!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(crate::tui::state::ActivePendingInteractionKind::PlanApproval)
    );
    assert!(app.pending_command_approval().is_some());
    assert!(agent_slot.is_some());
    assert!(app.bottom_pane.running_task.is_none());
}
