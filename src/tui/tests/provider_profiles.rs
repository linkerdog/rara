use super::*;

#[tokio::test]
async fn openai_model_picker_edit_shortcut_starts_wizard_for_selected_profile() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "custom-default",
        "Custom endpoint",
        OpenAiEndpointKind::Custom,
    );
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.model_picker_idx = 1;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::EditOpenAiProfile,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("open selected profile model editor");

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("custom-default")
    );
    assert!(matches!(app.overlay, Some(Overlay::BaseUrlEditor)));
    assert_eq!(
        app.openai_setup_steps,
        vec![
            Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible),
            Overlay::ModelNameEditor
        ]
    );
}

#[tokio::test]
async fn openai_profile_edit_wizard_keeps_existing_api_key_when_blank() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.config.set_api_key("sk-openrouter");
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::EditOpenAiProfile,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("start edit wizard");
    assert!(matches!(app.overlay, Some(Overlay::BaseUrlEditor)));

    dispatch_event(
        AppEvent::SaveBaseUrlInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save base url");
    assert!(matches!(
        app.overlay,
        Some(Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible))
    ));
    assert!(app.api_key_input.is_empty());

    dispatch_event(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("skip api key");

    assert_eq!(app.config.api_key(), Some("sk-openrouter"));
    assert!(matches!(app.overlay, Some(Overlay::ModelNameEditor)));
}

#[tokio::test]
async fn openai_model_picker_create_shortcut_opens_endpoint_kind_picker() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::CreateOpenAiProfile,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("open endpoint kind picker");

    assert!(matches!(app.overlay, Some(Overlay::ListPicker(_))));
    assert_eq!(app.openai_endpoint_kind_picker_idx, 0);
}

#[tokio::test]
async fn selecting_custom_endpoint_kind_prompts_for_profile_label() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.overlay = Some(Overlay::ListPicker(ListPickerKind::OpenAiEndpointKind));
    app.openai_endpoint_kind_picker_idx = 0;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("select endpoint kind");

    assert!(matches!(
        app.overlay,
        Some(Overlay::OpenAiProfileLabelEditor)
    ));
    assert_eq!(
        app.openai_profile_label_kind,
        Some(OpenAiEndpointKind::Custom)
    );
}

#[tokio::test]
async fn selecting_openai_profile_from_picker_switches_active_profile() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );
    app.config.select_openai_profile(
        "openrouter-backup",
        "OpenRouter Backup",
        OpenAiEndpointKind::Openrouter,
    );
    app.model_picker_idx = 3;
    app.open_overlay(Overlay::ListPicker(ListPickerKind::OpenAiProfile));
    app.openai_profile_picker_idx = 2;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply profile selection");

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-main")
    );
    assert!(matches!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::Model))
    ));
}

#[tokio::test]
async fn saving_openai_profile_label_creates_new_openrouter_profile() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::OpenAiProfile));
    app.openai_profile_picker_idx = 0;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("open profile label editor");

    app.openai_profile_label_input = "OpenRouter backup".to_string();

    dispatch_event(
        AppEvent::SaveOpenAiProfileLabelInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save profile label");

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-openrouter-backup")
    );
    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Openrouter)
    );
    assert!(
        app.config
            .openai_profiles
            .contains_key("openrouter-openrouter-backup")
    );
    assert!(matches!(
        app.overlay,
        Some(Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible))
    ));
    assert_eq!(app.openai_setup_steps, vec![Overlay::ModelNameEditor]);
}

#[tokio::test]
async fn save_api_key_input_allows_clearing_openai_compatible_credentials() {
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

    app.config.set_provider("openai-compatible");
    app.config.set_api_key("sk-existing");
    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible));
    app.api_key_input.clear();

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    let should_quit = dispatch_event(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save api key");

    assert!(!should_quit);
    assert_eq!(app.config.api_key(), None);
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Cleared API key"))
    );
}
