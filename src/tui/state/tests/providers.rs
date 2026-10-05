use super::*;

#[test]
fn openai_compatible_preset_sets_default_connection_fields() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    assert_eq!(
        app.selected_provider_family(),
        ProviderFamily::OpenAiCompatible
    );

    app.select_local_model(0);

    assert_eq!(app.config.provider, "openai-compatible");
    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Custom)
    );
    assert_eq!(app.config.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(
        app.config.base_url.as_deref(),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(app.config.revision, None);
}

#[test]
fn openai_compatible_preset_preserves_custom_model_name() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.set_provider("openai-compatible");
    app.config.set_model(Some("custom-model".to_string()));
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);

    app.select_local_model(0);

    assert_eq!(app.config.provider, "openai-compatible");
    assert_eq!(app.config.model.as_deref(), Some("custom-model"));
}

#[test]
fn deepseek_family_selects_deepseek_profile_and_model() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.select_local_model(0);
    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Deepseek)
    );
    assert_eq!(
        app.config.base_url.as_deref(),
        Some("https://api.deepseek.com/v1")
    );
    assert_eq!(app.config.model.as_deref(), Some("deepseek-flash"));
}

#[test]
fn deepseek_catalog_options_keep_current_custom_model_selectable() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config
        .set_model(Some("deepseek-v4-preview".to_string()));

    app.set_deepseek_model_options(vec!["deepseek-chat".to_string()]);

    assert!(
        app.deepseek_model_options
            .iter()
            .any(|model| model == "deepseek-v4-preview")
    );
    assert_eq!(app.model_picker_idx, app.selected_preset_idx());
    assert_eq!(
        app.deepseek_model_options
            .get(app.model_picker_idx)
            .map(String::as_str),
        Some("deepseek-v4-preview")
    );
}

#[test]
fn provider_catalog_context_window_flows_into_unified_model_presets() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let app = TuiApp::new(cm).expect("app");

    let presets = app.all_unified_model_presets();
    let kimi_k3 = presets
        .iter()
        .find(|preset| preset.provider_id == "kimi" && preset.model_id == "kimi-k3")
        .expect("Kimi K3 catalog entry");

    assert_eq!(kimi_k3.context_window, Some(1_048_576));
}

#[test]
fn available_unified_model_presets_exclude_unconfigured_remote_providers() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config
        .select_openai_profile("kimi-default", "Moonshot AI", OpenAiEndpointKind::Kimi);
    app.config.set_api_key("test-kimi-key");
    app.refresh_provider_connection_status();

    let presets = app.available_unified_model_presets();
    assert!(presets.iter().any(|preset| preset.provider_id == "kimi"));
    assert!(
        !presets
            .iter()
            .any(|preset| preset.provider_id == "deepseek")
    );
}

#[test]
fn model_catalog_snapshot_hydrates_provider_picker_state() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    let snapshot = ModelCatalogSnapshot {
        provider_id: "kimi".to_string(),
        models: vec![ModelCatalogEntry {
            id: "kimi-runtime-model".to_string(),
            context_window: Some(131_072),
        }],
        is_fallback: false,
    };
    app.apply_model_catalog_snapshots(&[snapshot]);

    assert_eq!(app.kimi_model_options, vec!["kimi-runtime-model"]);
    assert_eq!(
        app.model_context_window(ProviderFamily::Kimi, "kimi-runtime-model"),
        Some(131_072)
    );
}

#[test]
fn model_routing_view_infers_deepseek_auxiliary_model() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_model(Some("deepseek-v4-pro".to_string()));

    let routing = app.model_routing_view();

    assert_eq!(routing.main_model, "deepseek-v4-pro");
    assert_eq!(routing.auxiliary_model, "deepseek-flash");
    assert_eq!(routing.auxiliary_route, "provider_lite");
    assert_eq!(routing.auxiliary_source, "inferred");
    assert!(!routing.auxiliary_uses_main_model);
}

#[test]
fn model_routing_view_falls_back_to_main_model_without_helper() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.set_provider("ollama");
    app.config.set_model(Some("qwen3".to_string()));

    let routing = app.model_routing_view();

    assert_eq!(routing.main_model, "qwen3");
    assert_eq!(routing.auxiliary_model, "qwen3");
    assert_eq!(routing.auxiliary_route, "fallback");
    assert_eq!(routing.auxiliary_source, "main_model");
    assert!(routing.auxiliary_uses_main_model);
}

#[test]
fn terminal_diagnostics_view_uses_live_tui_dimensions() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.terminal_width = 123;
    app.terminal_focused = false;

    let terminal = app.terminal_diagnostics_view();

    assert_eq!(terminal.width_columns, 123);
    assert!(!terminal.focused);
    assert!(!terminal.user_agent.is_empty());
    assert!(!terminal.history_mode.is_empty());
}

#[test]
fn codex_preset_keeps_the_codex_model_label() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = 0;
    app.set_codex_model_options(vec![CodexModelOption {
        id: DEFAULT_CODEX_MODEL.to_string(),
        model: DEFAULT_CODEX_MODEL.to_string(),
        label: "gpt-5.4".to_string(),
        description: "Latest frontier agentic coding model.".to_string(),
        reasoning_options: vec![CodexReasoningOption {
            value: "medium".to_string(),
            label: "Medium".to_string(),
            description: "Default reasoning effort.".to_string(),
            is_default: true,
        }],
        default_reasoning_effort: Some("medium".to_string()),
        is_default: true,
    }]);
    app.select_local_model(0);

    assert_eq!(app.config.provider, "codex");
    assert_eq!(app.config.model.as_deref(), Some(DEFAULT_CODEX_MODEL));
    assert_eq!(app.config.base_url.as_deref(), Some(DEFAULT_CODEX_BASE_URL));
}

#[test]
fn opening_openai_compatible_model_picker_restores_provider_scoped_state() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.set_provider("openai-compatible");
    app.config
        .set_base_url(Some("http://proxy.local/v1".to_string()));
    app.config.set_model(Some("custom-model".to_string()));
    app.config.set_provider("codex");
    app.config.set_model(Some("codex".to_string()));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(app.config.provider, "openai-compatible");
    assert_eq!(
        app.config.base_url.as_deref(),
        Some("http://proxy.local/v1")
    );
    assert_eq!(app.config.model.as_deref(), Some("custom-model"));
}

#[test]
fn opening_openai_compatible_model_picker_excludes_deepseek_profile_kind() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_model(Some("deepseek-reasoner".to_string()));
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Custom)
    );
    assert_eq!(app.config.model.as_deref(), Some("gpt-4o-mini"));
}

#[test]
fn openai_compatible_model_picker_selects_profile_rows() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(app.current_model_picker_len(), 1);
    assert_eq!(app.model_picker_idx, 0);

    assert_eq!(
        app.selected_openai_model_picker_action(),
        Some(crate::tui::state::OpenAiModelPickerAction::SelectProfile)
    );

    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    assert_eq!(app.current_model_picker_len(), 2);

    app.model_picker_idx = 1;
    assert_eq!(
        app.selected_openai_model_picker_action(),
        Some(crate::tui::state::OpenAiModelPickerAction::SelectProfile)
    );
}

#[test]
fn openai_compatible_model_picker_deletes_active_profile_and_keeps_next() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

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

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-default")
    );
    assert_eq!(
        app.delete_active_openai_profile().as_deref(),
        Some("OpenRouter")
    );
    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("custom-default")
    );
    assert_eq!(app.model_picker_idx, 0);
    assert_eq!(app.current_model_picker_len(), 1);
}

#[test]
fn openai_profile_active_state_survives_switching_to_codex_and_ollama() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );
    app.config
        .set_model(Some("anthropic/claude-3.7-sonnet".to_string()));
    app.config.set_api_key("sk-openrouter");
    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-main")
    );

    app.provider_picker_idx = 0;
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.select_local_model(0);
    assert_eq!(app.config.provider, "codex");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::Ollama);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.select_local_model(0);
    assert_eq!(app.config.provider, "ollama");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-main")
    );
    assert_eq!(
        app.config.model.as_deref(),
        Some("anthropic/claude-3.7-sonnet")
    );
    assert_eq!(app.model_picker_idx, 0);
}

#[test]
fn opening_openai_profile_picker_prefers_active_profile_of_selected_kind() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.model_picker_idx = 4;

    app.open_overlay(Overlay::ListPicker(ListPickerKind::OpenAiProfile));

    assert_eq!(
        app.selected_openai_profile_kind(),
        Some(OpenAiEndpointKind::Openrouter)
    );
    assert_eq!(app.openai_profile_picker_idx, 1);
}

#[test]
fn openai_model_selection_keeps_non_default_profile_for_same_kind() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );

    app.select_local_model(4);

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-main")
    );
    assert_eq!(
        app.config.active_openai_profile_label(),
        Some("OpenRouter Main")
    );
}

#[test]
fn model_name_editor_seeds_from_selected_provider_state() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.set_provider("openai-compatible");
    app.config.set_model(Some("custom-model".to_string()));
    app.config.set_provider("codex");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.open_overlay(Overlay::ModelNameEditor);

    assert_eq!(app.model_name_input, "custom-model");
}

#[test]
fn model_name_editor_does_not_panic_when_provider_has_no_presets() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    // DeepSeek has empty presets (&[])
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.open_overlay(Overlay::ModelNameEditor);

    // Should not panic, and model_name_input stays empty since
    // config.model is None and there is no preset to fall back to.
    assert_eq!(app.model_name_input, "");
}

#[test]
fn closing_auth_mode_picker_with_empty_stack_returns_to_none() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.open_overlay(Overlay::ListPicker(ListPickerKind::AuthMode));
    app.dismiss_overlay();

    // Stack-based back-navigation: closing the only overlay returns to None.
    assert!(app.overlay.is_none());
}
