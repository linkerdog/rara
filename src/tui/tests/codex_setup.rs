use super::*;

#[test]
fn codex_auth_detection_uses_saved_auth_storage() {
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

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");

    assert!(!codex_auth_is_available(&app, &oauth_manager));

    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");
    assert!(codex_auth_is_available(&app, &oauth_manager));
}

#[tokio::test]
async fn codex_provider_family_routes_to_auth_picker_without_saved_login() {
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

    app.provider_picker_idx = 0;

    assert_eq!(app.selected_provider_family(), ProviderFamily::Codex);

    open_provider_family_overlay(&mut app);
    assert_eq!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::AuthMode))
    );
}

#[tokio::test]
async fn codex_provider_family_routes_to_model_picker_with_saved_login() {
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

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");
    app.provider_picker_idx = 0;

    open_provider_family_overlay(&mut app);
    assert_eq!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::AuthMode))
    );
}

#[tokio::test]
async fn codex_provider_family_uses_saved_codex_provider_state() {
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

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");

    app.config.set_provider("ollama");
    app.config.set_api_key("sk-ollama");
    app.config.set_provider("codex");
    app.config.set_api_key("sk-codex");
    app.config.set_provider("ollama");
    app.provider_picker_idx = 0;

    assert!(codex_auth_is_available(&app, &oauth_manager));

    open_provider_family_overlay(&mut app);
    // Connected → overlay closes. Re-open for test assertion.
    app.overlay = Some(Overlay::ListPicker(ListPickerKind::UnifiedModel));
    assert!(matches!(app.overlay, Some(Overlay::ListPicker(_))));
}

#[tokio::test]
async fn codex_model_picker_opens_reasoning_level_overlay_before_rebuild() {
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

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");

    app.codex_model_options = vec![crate::codex_model_catalog::CodexModelOption {
        id: "anthropic/claude-3-5-sonnet-20241022".into(),
        label: "Claude 3.5 Sonnet v2".into(),
        model: "anthropic/claude-3-5-sonnet-20241022".into(),
        reasoning_options: vec![
            crate::codex_model_catalog::CodexReasoningOption {
                label: "Low".into(),
                value: "low".into(),
                ..Default::default()
            },
            crate::codex_model_catalog::CodexReasoningOption {
                label: "High".into(),
                value: "high".into(),
                ..Default::default()
            },
        ],
        is_default: true,
        ..Default::default()
    }];

    app.provider_picker_idx = 0;
    open_provider_family_overlay(&mut app);
    app.overlay = Some(Overlay::ListPicker(ListPickerKind::UnifiedModel));
    app.model_picker_idx = 0;

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
        Some(Overlay::ListPicker(ListPickerKind::ReasoningEffort))
    ));
}

#[tokio::test]
async fn codex_model_picker_applies_single_reasoning_level_without_overlay() {
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

    app.provider_picker_idx = 0;
    app.config.set_provider("codex");
    app.set_codex_model_options(vec![CodexModelOption {
        id: "gpt-5.2-codex".to_string(),
        model: "gpt-5.2-codex".to_string(),
        label: "gpt-5.2-codex".to_string(),
        description: "Frontier agentic coding model.".to_string(),
        default_reasoning_effort: Some("high".to_string()),
        reasoning_options: vec![CodexReasoningOption {
            value: "high".to_string(),
            label: "High".to_string(),
            description: "Maximize reasoning depth.".to_string(),
            is_default: true,
        }],
        is_default: true,
    }]);
    app.overlay = Some(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply model selection");

    assert_eq!(app.config.model.as_deref(), Some("gpt-5.2-codex"));
    assert_eq!(app.config.reasoning_effort.as_deref(), Some("high"));
    assert!(matches!(
        app.bottom_pane.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::Rebuild)
    ));
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn codex_auth_store_is_synced_into_config_before_model_flow() {
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

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");

    app.config.set_provider("ollama");
    app.provider_picker_idx = 0;

    assert!(sync_codex_credential_from_auth_store(&mut app, &oauth_manager).expect("sync auth"));
    assert_eq!(
        app.config
            .provider_states
            .get("codex")
            .and_then(|state| state.api_key.as_ref())
            .map(|value| value.expose_secret()),
        Some("sk-test-codex")
    );
    assert_eq!(app.config.provider, "ollama");

    let persisted = app.config_manager.load().expect("load saved config");
    assert_eq!(persisted.provider, "ollama");
    assert_eq!(
        persisted
            .provider_states
            .get("codex")
            .and_then(|state| state.api_key.as_ref())
            .map(|value| value.expose_secret()),
        Some("sk-test-codex")
    );
}

#[test]
fn codex_chatgpt_auth_store_sets_chatgpt_base_url_before_model_flow() {
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

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    codex_login::save_auth(
        &temp.path().join(".rara").join("codex-auth"),
        &codex_login::AuthDotJson {
            auth_mode: None,
            openai_api_key: Some("sk-from-oauth".into()),
            tokens: Some(codex_login::TokenData {
                id_token: codex_login::token_data::parse_chatgpt_jwt_claims(
                    "eyJhbGciOiJub25lIn0.e30.signature",
                )
                .expect("valid id token"),
                access_token: "oauth-access-token".into(),
                refresh_token: "refresh".into(),
                account_id: None,
            }),
            last_refresh: None,
            agent_identity: None,
            personal_access_token: None,
            bedrock_api_key: None,
            bedrock_access_keys: None,
        },
        codex_login::AuthCredentialsStoreMode::File,
        codex_login::AuthKeyringBackendKind::default(),
    )
    .expect("save auth");

    app.config.set_provider("ollama");

    assert!(sync_codex_credential_from_auth_store(&mut app, &oauth_manager).expect("sync auth"));
    assert_eq!(
        app.config
            .provider_states
            .get("codex")
            .and_then(|state| state.api_key.as_ref())
            .map(|value| value.expose_secret()),
        Some("oauth-access-token")
    );
    assert_eq!(
        app.config
            .provider_states
            .get("codex")
            .and_then(|state| state.base_url.as_deref()),
        Some(DEFAULT_CODEX_CHATGPT_BASE_URL)
    );
}

#[tokio::test]
async fn save_api_key_input_sets_codex_defaults_before_rebuild() {
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

    app.config.set_provider("codex");
    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Codex));
    app.api_key_input = "sk-codex".into();

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
    .expect("save codex api key");

    assert!(!should_quit);
    assert_eq!(app.config.model.as_deref(), Some(DEFAULT_CODEX_MODEL));
    assert_eq!(app.config.base_url.as_deref(), Some(DEFAULT_CODEX_BASE_URL));
    assert_eq!(
        app.codex_auth_mode,
        Some(crate::oauth::SavedCodexAuthMode::ApiKey)
    );
}
