use super::*;

#[tokio::test]
async fn slash_palette_model_selection_opens_provider_picker_in_local_and_ssh() {
    for ssh in [false, true] {
        let temp = tempdir().expect("tempdir");
        let _ssh_env = crate::tui::terminal_ui::test_env::set_ssh_session(ssh);

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

        app.set_input("/".to_string());
        let model_idx = palette_commands(&app, "")
            .iter()
            .position(|spec| spec.name == "model")
            .expect("model command present");
        app.command_palette_idx = model_idx;

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
        .expect("apply command palette selection");

        assert!(
            matches!(app.overlay, Some(Overlay::ModelSearch)),
            "model search should open after model selection (ssh={ssh}), \
             but overlay was {overlay:?}",
            ssh = ssh,
            overlay = app.overlay,
        );
    }
}

#[tokio::test]
async fn mem_picker_persists_selected_cloud_mode() {
    let temp = tempdir().expect("tempdir");
    let config_path = temp.path().join("config.json");
    let mut app = TuiApp::new(ConfigManager {
        path: config_path.clone(),
    })
    .expect("build tui app");
    app.open_overlay(Overlay::ListPicker(ListPickerKind::NowledgeMem));
    app.nowledge_mem_picker_idx = 2;

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
    .expect("apply memory mode selection");

    assert!(app.config.builtin_plugins.nowledge_mem.enabled);
    assert_eq!(
        app.config.builtin_plugins.nowledge_mem.mode,
        crate::config::NowledgeMemMode::Cloud
    );
    assert_eq!(
        app.config.builtin_plugins.nowledge_mem.url,
        crate::config::DEFAULT_NOWLEDGE_MEM_CLOUD_URL
    );
    let saved = std::fs::read_to_string(config_path).expect("saved config");
    assert!(saved.contains("cloud"));
}

#[tokio::test]
async fn mem_picker_preserves_existing_cloud_url() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.builtin_plugins.nowledge_mem.mode = crate::config::NowledgeMemMode::Cloud;
    app.config.builtin_plugins.nowledge_mem.url = "https://custom.mem.example".to_string();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::NowledgeMem));
    app.nowledge_mem_picker_idx = 2;

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
    .expect("apply memory mode selection");

    assert_eq!(
        app.config.builtin_plugins.nowledge_mem.url,
        "https://custom.mem.example"
    );
}

#[test]
fn provider_picker_number_keys_cover_current_provider_families() {
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

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Provider));

    let key_char =
        char::from_digit(crate::tui::state::PROVIDER_FAMILIES.len() as u32, 10).expect("digit key");
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char(key_char)), &app),
        AppEvent::SetListPickerSelection(idx)
            if idx == crate::tui::state::PROVIDER_FAMILIES.len() - 1
    ));
}

#[test]
fn auth_mode_picker_prefers_selection_navigation() {
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

    app.open_overlay(Overlay::ListPicker(ListPickerKind::AuthMode));

    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::MoveListPickerSelection(1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Enter), &app),
        AppEvent::ApplyOverlaySelection
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('3')), &app),
        AppEvent::SetListPickerSelection(2)
    ));
}

#[test]
fn app_starts_with_warning_instead_of_api_key_editor_for_hosted_provider_without_api_key() {
    let temp = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: temp.path().join("config.json"),
    };
    let mut config = cm.load().expect("load config");
    config.set_provider("openai-compatible");
    config.clear_api_key();
    cm.save(&config).expect("save config");

    let app = TuiApp::new(cm).expect("app");
    assert!(app.overlay.is_none());
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.starts_with("Warning:"))
    );
}

#[tokio::test]
async fn openai_model_picker_delete_row_removes_active_profile() {
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
    app.config.set_api_key("sk-custom");
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::DeleteOpenAiProfile,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("delete profile");

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("custom-default")
    );
    assert!(matches!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::Model))
    ));
    assert!(matches!(
        app.bottom_pane.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::Rebuild)
    ));
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn openai_model_picker_space_activates_selected_profile_and_starts_setup_when_incomplete() {
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
        AppEvent::SetListPickerSelection(0),
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("set model selection");

    assert!(matches!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::Model))
    ));

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("activate selected profile");

    assert!(matches!(app.overlay, Some(Overlay::BaseUrlEditor)));
}

#[tokio::test]
async fn deepseek_provider_family_prompts_for_api_key_before_model_list() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);

    open_provider_family_overlay(&mut app);

    assert_eq!(
        app.overlay,
        Some(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek))
    );
}

#[tokio::test]
async fn deepseek_api_key_save_starts_model_catalog_task() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek));
    app.api_key_input = "sk-deepseek-test".to_string();

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save api key");

    assert_eq!(app.config.api_key(), Some("sk-deepseek-test"));
    assert!(matches!(
        app.bottom_pane.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::ModelCatalog)
    ));
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn moonshot_connection_saves_to_moonshot_without_overwriting_active_codex_key() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.set_provider("codex");
    app.config.set_api_key("sk-codex");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::Kimi);

    open_provider_family_overlay(&mut app);

    assert_eq!(app.overlay, Some(Overlay::ApiKeyEditor(ApiKeyTarget::Kimi)));
    app.api_key_input = "sk-moonshot".to_string();
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save Moonshot API key");

    assert_eq!(app.config.provider, "codex");
    assert_eq!(app.config.api_key(), Some("sk-codex"));
    let kimi_profile = app
        .config
        .openai_profiles
        .get(OpenAiEndpointKind::Kimi.default_profile_id())
        .expect("Moonshot profile");
    assert_eq!(
        kimi_profile
            .api_key
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("sk-moonshot")
    );
    assert!(matches!(
        app.bottom_pane.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::ModelCatalog)
    ));
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn kimi_coding_connection_uses_the_dedicated_profile_and_endpoint() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.set_provider("codex");
    app.config.set_api_key("sk-codex");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::KimiCoding);

    open_provider_family_overlay(&mut app);

    assert_eq!(
        app.overlay,
        Some(Overlay::ApiKeyEditor(ApiKeyTarget::KimiCoding))
    );
    app.api_key_input = "sk-kimi-code".to_string();
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let runtime = FakeRuntimeClient::new(Default::default());
    let mut agent_slot = None;

    dispatch_event_with_runtime(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
        &runtime,
    )
    .await
    .expect("save Kimi For Coding API key");

    assert_eq!(app.config.provider, "openai-compatible");
    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::KimiCoding)
    );
    assert_eq!(app.config.api_key(), Some("sk-kimi-code"));
    assert_eq!(
        app.config.base_url.as_deref(),
        Some(crate::config::DEFAULT_KIMI_CODING_BASE_URL)
    );
    let profile = app
        .config
        .openai_profiles
        .get(OpenAiEndpointKind::KimiCoding.default_profile_id())
        .expect("Kimi For Coding profile");
    assert_eq!(
        profile.api_key.as_ref().map(ExposeSecret::expose_secret),
        Some("sk-kimi-code")
    );
    assert_eq!(
        profile.base_url.as_deref(),
        Some(crate::config::DEFAULT_KIMI_CODING_BASE_URL)
    );
    assert_eq!(
        profile.model.as_deref(),
        Some(crate::config::DEFAULT_KIMI_CODING_MODEL)
    );
    assert_eq!(
        app.config
            .provider_states
            .get("codex")
            .and_then(|state| state.api_key.as_ref())
            .map(ExposeSecret::expose_secret),
        Some("sk-codex")
    );
    assert_eq!(
        app.bottom_pane.notice.as_deref(),
        Some("Saved Kimi For Coding API key. Rebuilding backend.")
    );
    assert!(app.bottom_pane.running_task.is_none());
    assert_eq!(
        runtime.commands(),
        vec![RuntimeCommand::Maintenance(
            RuntimeMaintenanceCommand::Rebuild
        )]
    );
}

#[tokio::test]
async fn deepseek_model_picker_enter_without_api_key_opens_api_key_editor() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.clear_api_key();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

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
    .expect("apply model selection");

    assert!(matches!(
        app.overlay,
        Some(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek))
    ));
    assert!(app.bottom_pane.running_task.is_none());
}

#[tokio::test]
async fn deepseek_model_picker_api_key_action_opens_editor_even_when_key_exists() {
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

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_api_key("sk-deepseek-test");
    app.set_deepseek_model_options(vec!["deepseek-chat".to_string()]);
    app.model_picker_idx = app.deepseek_api_key_action_idx();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

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
    .expect("apply api key action");

    assert!(matches!(
        app.overlay,
        Some(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek))
    ));
    assert!(app.bottom_pane.running_task.is_none());
}
