use super::*;

#[test]
fn request_input_shortcuts_match_advertised_three_options() {
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

    add_pending_request_input(&mut app, 4);

    assert_eq!(app.active_pending_option_count(), 3);
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('3')), &app),
        AppEvent::SelectPendingOption(2)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('4')), &app),
        AppEvent::InputChar('4')
    ));
}

#[test]
fn plain_input_does_not_treat_s_as_setup_shortcut() {
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

    app.bottom_pane.input = "先同步ma".into();

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('s')), &app),
        AppEvent::InputChar('s')
    ));
}

#[test]
fn shift_enter_inserts_newline_in_main_composer() {
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

    assert!(matches!(
        map_key_to_event(shifted_key(KeyCode::Enter), &app),
        AppEvent::InsertNewline
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Enter), &app),
        AppEvent::SubmitComposer
    ));
}

#[test]
fn arrow_keys_and_home_end_map_to_composer_cursor_events() {
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

    app.bottom_pane.input = "hello".into();

    assert!(matches!(
        map_key_to_event(key(KeyCode::Left), &app),
        AppEvent::MoveCursorLeft
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Right), &app),
        AppEvent::MoveCursorRight
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Home), &app),
        AppEvent::MoveCursorHome
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::End), &app),
        AppEvent::MoveCursorEnd
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::MoveCursorUp
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::MoveCursorDown
    ));
}

#[test]
fn empty_composer_uses_up_down_for_input_history_when_available() {
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

    app.record_input_history("previous request");

    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::NavigateInputHistory(-1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::NavigateInputHistory(1)
    ));
}

#[test]
fn empty_composer_keeps_j_and_k_as_text() {
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

    app.record_input_history("previous request");

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('k')), &app),
        AppEvent::InputChar('k')
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('j')), &app),
        AppEvent::InputChar('j')
    ));
}

#[test]
fn input_history_navigation_recalls_previous_submissions_and_restores_draft() {
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

    app.record_input_history("first request");
    app.record_input_history("second request");
    app.set_input("draft".to_string());

    app.navigate_input_history(-1);
    assert_eq!(app.bottom_pane.input, "second request");
    assert_eq!(
        app.composer_cursor_offset(),
        "second request".chars().count()
    );

    app.navigate_input_history(-1);
    assert_eq!(app.bottom_pane.input, "first request");

    app.navigate_input_history(1);
    assert_eq!(app.bottom_pane.input, "second request");

    app.navigate_input_history(1);
    assert_eq!(app.bottom_pane.input, "draft");
    assert_eq!(app.input_history_cursor, None);
}

#[test]
fn input_history_navigation_starts_from_non_empty_draft_at_start() {
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

    app.record_input_history("previous request");
    app.set_input("draft".to_string());
    app.bottom_pane.input_cursor_offset = Some(0);

    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::NavigateInputHistory(-1)
    ));

    app.navigate_input_history(-1);
    assert_eq!(app.bottom_pane.input, "previous request");
    app.navigate_input_history(1);
    assert_eq!(app.bottom_pane.input, "draft");
}

#[test]
fn input_history_keeps_recent_entries_bounded() {
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

    for idx in 0..250 {
        app.record_input_history(&format!("request {idx}"));
    }

    assert_eq!(app.input_history.len(), 200);
    assert_eq!(
        app.input_history.first().map(String::as_str),
        Some("request 50")
    );
    assert_eq!(
        app.input_history.last().map(String::as_str),
        Some("request 249")
    );
}

#[test]
fn input_history_navigation_keeps_multiline_cursor_movement_for_unrecalled_text() {
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

    app.record_input_history("previous request");
    app.set_input("line one\nline two".to_string());
    app.bottom_pane.input_cursor_offset = Some("line one\nline".chars().count());

    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::MoveCursorUp
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::MoveCursorDown
    ));
}
