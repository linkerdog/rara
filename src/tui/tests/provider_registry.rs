use super::*;

#[tokio::test]
async fn configured_provider_connect_preserves_active_model_and_exposes_named_models() {
    let temp = tempdir().expect("tempdir");
    std::fs::create_dir(temp.path().join(".git")).expect("git boundary");
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home")).expect("config root");
    let config = manager.load_for_project_with_env(temp.path(), &|key| {
        (key == "RARA_CONFIG_CONTENT").then(|| r#"{"provider":{"groq":{"models":{"alias":{"id":"org/model","name":"Named model","limit":{"context":32000}}}},"xai":{"models":{"unconnected":{}}}}}"#.into())
    }).expect("provider config");
    let mut app = TuiApp::with_config(manager, config).expect("app");
    assert!(
        app.available_unified_model_presets()
            .iter()
            .all(|model| model.provider_id != "groq")
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Provider));
    app.provider_picker_idx = crate::tui::state::PROVIDER_FAMILIES.len();
    open_provider_family_overlay(&mut app);
    assert_eq!(app.registry_credential_target.as_deref(), Some("groq"));
    assert_eq!(
        app.overlay,
        Some(Overlay::ApiKeyEditor(ApiKeyTarget::Registry))
    );
    // Navigation changes cannot redirect the captured credential target.
    app.provider_picker_idx += 1;
    app.api_key_input = "groq-fixture-key".into();
    let oauth = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join("home")).expect("oauth"),
    );
    dispatch_event(AppEvent::SaveApiKeyInput, &mut app, &mut None, &oauth)
        .await
        .expect("save key");
    assert_eq!(app.config.provider, "mock");
    assert!(app.config.api_key().is_none());
    let models = app.available_unified_model_presets();
    let configured = models
        .iter()
        .find(|model| model.provider_id == "groq")
        .expect("configured model");
    assert_eq!(configured.model_label, "Named model");
    assert_eq!(configured.model_id, "alias");
    assert_eq!(configured.context_window, Some(32000));
    assert!(models.iter().all(|model| model.provider_id != "xai"));
    let index = app
        .all_unified_model_presets()
        .iter()
        .position(|model| model.provider_id == "groq")
        .expect("model index");
    app.select_unified_model(index);
    assert_eq!(app.config.provider, "groq");
    assert_eq!(
        app.selected_provider_family(),
        ProviderFamily::OpenAiCompatible
    );
    assert_eq!(app.config.model.as_deref(), Some("org/model"));
    assert_eq!(app.config.api_key(), Some("groq-fixture-key"));
    app.config_manager
        .save(&app.config)
        .expect("save selection");
    let saved = std::fs::read_to_string(&app.config_manager.path).expect("legacy config");
    assert!(!saved.contains("groq-fixture-key"));
}
