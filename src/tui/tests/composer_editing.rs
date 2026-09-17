use super::*;

#[test]
fn mouse_wheel_scrolls_transcript() {
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

    // First scroll without prior events → base 3 lines (factor 1.0).
    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => {
            assert!((-15..=-3).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
    match translate_event(mouse_scroll(MouseEventKind::ScrollDown), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => {
            assert!((3..=15).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[test]
fn mouse_wheel_with_command_palette_routes_to_move_command_selection() {
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

    app.open_overlay(Overlay::CommandPalette);

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::MoveCommandSelection(delta))) => {
            assert!(delta < 0, "delta {delta} should be negative");
        }
        event => panic!("unexpected event: {event:?}"),
    }

    match translate_event(mouse_scroll(MouseEventKind::ScrollDown), &app) {
        Some(UiEvent::App(AppEvent::MoveCommandSelection(delta))) => {
            assert!(delta > 0, "delta {delta} should be positive");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[tokio::test]
async fn composer_supports_mid_input_insertion_and_backspace() {
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

    app.set_input("helo".to_string());

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::MoveCursorLeft,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("move left");
    dispatch_event(
        AppEvent::InputChar('l'),
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("insert");
    assert_eq!(app.bottom_pane.input, "hello");
    assert_eq!(app.composer_cursor_offset(), 4);

    dispatch_event(
        AppEvent::Backspace,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("backspace");
    assert_eq!(app.bottom_pane.input, "helo");
    assert_eq!(app.composer_cursor_offset(), 3);
}

#[tokio::test]
async fn paste_inserts_at_current_cursor_offset() {
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

    app.set_input("helo".to_string());
    app.move_active_input_cursor_left();

    crate::tui::terminal_ui::handle_paste("l".to_string(), &mut app);

    assert_eq!(app.bottom_pane.input, "hello");
    assert_eq!(app.composer_cursor_offset(), 4);
}

#[tokio::test]
async fn paste_normalizes_crlf_and_cr_newlines() {
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

    crate::tui::terminal_ui::handle_paste("first\r\nsecond\rthird".to_string(), &mut app);

    // Flush paste burst so the text actually lands in the input.
    app.bottom_pane.flush_paste_burst();

    assert_eq!(app.bottom_pane.input, "first\nsecond\nthird");
    assert_eq!(
        app.composer_cursor_offset(),
        "first\nsecond\nthird".chars().count()
    );
}

#[test]
fn large_paste_inserts_placeholder_at_cursor_position() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    // Pre-fill input and move cursor to middle
    app.bottom_pane.input = "before after".to_string();
    app.bottom_pane.input_cursor_offset = Some("before ".chars().count());

    let big = "x".repeat(1200);
    crate::tui::terminal_ui::handle_paste(big.clone(), &mut app);
    app.bottom_pane.flush_paste_burst();

    // Placeholder should appear at cursor position, not end
    assert!(
        app.bottom_pane
            .input
            .starts_with("before [Pasted Content #0 — 1200 chars]after")
    );
    // Cursor should be after the placeholder
    assert_eq!(
        app.composer_cursor_offset(),
        "before [Pasted Content #0 — 1200 chars]".chars().count()
    );
}

#[test]
fn multiple_large_pastes_accumulate_in_pending_vec() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let big_a = "a".repeat(1200);
    let big_b = "b".repeat(1100);
    crate::tui::terminal_ui::handle_paste(big_a.clone(), &mut app);
    app.bottom_pane.flush_paste_burst();
    assert_eq!(app.bottom_pane.large_paste_pending.len(), 1);

    crate::tui::terminal_ui::handle_paste(big_b.clone(), &mut app);
    app.bottom_pane.flush_paste_burst();
    assert_eq!(app.bottom_pane.large_paste_pending.len(), 2);

    // Both placeholders should be in the input
    assert!(app.bottom_pane.input.contains("Pasted Content #0"));
    assert!(app.bottom_pane.input.contains("Pasted Content #1"));
    // Counters should be unique
    assert_ne!(
        app.bottom_pane.large_paste_pending[0].0,
        app.bottom_pane.large_paste_pending[1].0
    );
}

#[test]
fn expand_large_paste_replaces_all_placeholders() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let big = "z".repeat(1500);
    crate::tui::terminal_ui::handle_paste(big.clone(), &mut app);
    app.bottom_pane.flush_paste_burst();
    assert!(app.bottom_pane.large_paste_pending.len() == 1);
    assert!(app.bottom_pane.input.contains("Pasted Content"));
    assert!(!app.bottom_pane.input.contains(&big));

    app.bottom_pane.expand_large_paste();

    // After expand: placeholder gone, full text present, counter reset
    assert!(!app.bottom_pane.input.contains("Pasted Content"));
    assert!(app.bottom_pane.input.contains(&big));
    assert_eq!(app.bottom_pane.large_paste_pending.len(), 0);
    assert_eq!(app.bottom_pane.large_paste_counter, 0);
}

#[test]
fn crossterm_paste_event_uses_paste_channel() {
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

    match translate_event(Event::Paste("first\nsecond".to_string()), &app) {
        Some(UiEvent::Paste(text)) => assert_eq!(text, "first\nsecond"),
        other => panic!("expected paste event, got {other:?}"),
    }
}

#[tokio::test]
async fn composer_supports_vertical_cursor_navigation_across_lines() {
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

    app.terminal_width = 12;
    app.set_input("abcd\nefgh".to_string());
    app.bottom_pane.input_cursor_offset = Some("abcd\nef".chars().count());

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::MoveCursorUp,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("move up");
    assert_eq!(app.composer_cursor_offset(), 2);

    dispatch_event(
        AppEvent::MoveCursorDown,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("move down");
    assert_eq!(app.composer_cursor_offset(), "abcd\nef".chars().count());
}
