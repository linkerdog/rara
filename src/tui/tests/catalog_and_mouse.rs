use super::*;

#[tokio::test]
async fn deepseek_model_picker_shows_dynamic_models_after_list_load() {
    use crate::config::OpenAiEndpointKind;

    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_api_key("sk-deepseek-test");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let initial_count = ListPickerKind::Model.item_count(&app);
    assert!(
        initial_count > 0,
        "initial model list should have fallback models"
    );

    app.set_deepseek_model_options(vec![
        "deepseek-flash".to_string(),
        "deepseek-v4-pro".to_string(),
    ]);
    let loaded_count = ListPickerKind::Model.item_count(&app);
    assert_eq!(
        loaded_count, 3,
        "after loading models, picker should show 2 models + 1 action"
    );

    app.dismiss_overlay();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    assert_eq!(
        ListPickerKind::Model.item_count(&app),
        3,
        "after reopening picker, still 2 models + 1 action"
    );
}

#[test]
fn mouse_wheel_with_no_overlay_routes_to_scroll_transcript() {
    let temp = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    assert!(app.overlay.is_none());

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => {
            assert!((-15..=0).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }

    match translate_event(mouse_scroll(MouseEventKind::ScrollDown), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => {
            assert!((0..=15).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[test]
fn left_mouse_drag_routes_to_transcript_selection_without_overlay() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let click = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 5,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });

    assert!(matches!(
        translate_event(click.clone(), &app),
        Some(UiEvent::App(AppEvent::StartTranscriptSelection(position)))
            if position.x == 5 && position.y == 10
    ));

    app.open_overlay(Overlay::CommandPalette);
    assert!(app.overlay.is_some());

    assert!(matches!(
        translate_event(click, &app),
        Some(UiEvent::App(AppEvent::Noop))
    ));
}

#[test]
fn mouse_wheel_with_status_overlay_routes_to_noop() {
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

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::Noop)) => {}
        event => panic!("unexpected event: {event:?}"),
    }
}

#[test]
fn mouse_wheel_with_context_overlay_routes_to_scroll_context() {
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

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollContext(delta))) => {
            assert!((-15..=0).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}
