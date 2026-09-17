use std::fs;

use serde_json::json;
use tempfile::tempdir;

use crate::{ConfigManager, split_model_reference};

#[test]
fn provider_layers_merge_models_and_preserve_secret_references_on_save() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let home = temp.path().join("home");
    let project = temp.path().join("repo");
    fs::create_dir_all(project.join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(home.clone())?;
    fs::write(
        home.join("rara.jsonc"),
        r#"{
        // Provider secrets are runtime values.
        "provider": {"groq": {"options": {"apiKey": "{env:TEST_KEY}"},
          "models": {"alias": {"id": "org/model", "name": "Global", "limit": {"context": 32000, "output": 500}}}}},
        "model": "groq/alias",
    }"#,
    )?;
    fs::write(
        project.join("rara.json"),
        r#"{"provider":{"groq":{"models":{"alias":{"name":"Project"},"other":{}}}}}"#,
    )?;
    let mut config = manager.load_for_project_with_env(&project, &|key| {
        (key == "TEST_KEY").then(|| "secret-\"value".into())
    })?;
    assert_eq!(config.provider, "groq");
    assert_eq!(config.model.as_deref(), Some("org/model"));
    assert_eq!(config.api_key(), Some("secret-\"value"));
    let model = config
        .selected_registry_model()
        .ok_or_else(|| anyhow::anyhow!("missing selection"))?;
    assert_eq!(model.name.as_deref(), Some("Project"));
    assert_eq!(model.limit.context, Some(32000));
    assert_eq!(
        config.provider_registry.document.provider["groq"]
            .models
            .len(),
        2
    );
    config.set_reasoning_effort(Some("high".into()));
    manager.save(&config)?;
    let saved = fs::read_to_string(&manager.path)?;
    assert!(!saved.contains("secret-"));
    assert!(!saved.contains("org/model"));
    assert!(fs::read_to_string(home.join("rara.jsonc"))?.contains("{env:TEST_KEY}"));
    assert_eq!(
        fs::read_to_string(home.join("model-selection.json"))?,
        "\"groq/alias\""
    );
    Ok(())
}

#[test]
fn explicit_project_and_inline_precedence_and_relative_files() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let home = temp.path().join("home");
    let project = temp.path().join("repo");
    fs::create_dir_all(project.join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(home.clone())?;
    let custom = home.join("explicit.json");
    fs::write(home.join("key.txt"), "file-key\n")?;
    fs::write(
        &custom,
        r#"{"provider":{"private":{"options":{"baseURL":"http://localhost:9191/api/v4","apiKey":"{file:key.txt}"},"models":{"one":{"name":"Custom"}}}}}"#,
    )?;
    fs::write(
        project.join("rara.json"),
        r#"{"provider":{"private":{"models":{"one":{"name":"Project"}}}}}"#,
    )?;
    let config = manager.load_for_project_with_env(&project, &|key| match key {
        "RARA_CONFIG" => Some(custom.display().to_string()),
        "RARA_CONFIG_CONTENT" => Some(r#"{"model":"private/one","provider":{"private":{"models":{"one":{"name":"Inline"}}}}}"#.into()),
        _ => None,
    })?;
    assert_eq!(config.api_key(), Some("file-key"));
    assert_eq!(
        config
            .selected_registry_model()
            .and_then(|model| model.name.as_deref()),
        Some("Inline")
    );
    assert_eq!(
        config.base_url.as_deref(),
        Some("http://localhost:9191/api/v4")
    );
    Ok(())
}

#[test]
fn filters_and_provider_credentials_are_isolated() -> anyhow::Result<()> {
    let temp = tempdir()?;
    fs::create_dir(temp.path().join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home"))?;
    let document = json!({"provider": {
        "groq": {"models":{"one":{},"hidden":{}},"whitelist":["one","hidden"],"blacklist":["hidden"]},
        "xai": {"models":{"one":{}}},
        "together": {"models":{"one":{}}}
    },"disabled_providers":["together"]}).to_string();
    let mut config = manager.load_for_project_with_env(temp.path(), &|key| match key {
        "RARA_CONFIG_CONTENT" => Some(document.clone()),
        "GROQ_API_KEY" => Some("groq-only".into()),
        "TOGETHER_API_KEY" => Some("disabled-key".into()),
        _ => None,
    })?;
    assert!(config.provider_registry.available("groq"));
    assert!(!config.provider_registry.available("xai"));
    assert!(!config.provider_registry.available("together"));
    assert!(config.select_registry_model("groq", "hidden").is_err());
    assert!(config.select_registry_model("xai", "one").is_err());
    assert_eq!(config.api_key(), Some("groq-only"));
    Ok(())
}

#[test]
fn unsupported_transports_and_unknown_options_fail_without_secret_diagnostics() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    fs::create_dir(temp.path().join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home"))?;
    for options in [
        json!({"apiKey":"secret-value","unknown":"secret-value"}),
        json!({"apiKey":123}),
    ] {
        let document = json!({"provider":{"groq":{"options":options}}}).to_string();
        let error = manager
            .load_for_project_with_env(temp.path(), &|key| {
                (key == "RARA_CONFIG_CONTENT").then(|| document.clone())
            })
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected invalid config"))?;
        assert!(!format!("{error:#}").contains("secret-value"));
    }
    let document = r#"{"provider":{"anthropic":{"npm":"@ai-sdk/anthropic"}}}"#;
    assert!(
        manager
            .load_for_project_with_env(temp.path(), &|key| (key == "RARA_CONFIG_CONTENT")
                .then(|| document.into()))
            .is_err()
    );
    Ok(())
}

#[test]
fn jsonc_preserves_urls_quotes_unicode_and_rejects_broken_comments() -> anyhow::Result<()> {
    let parsed = crate::provider_json::parse_document(
        r#"{/* note */"url":"https://example.test/v1", "text":"\u4e2d\u6587 \\\" // /*", "array":[1,/*end*/],}"#,
    )?;
    assert_eq!(parsed["url"], "https://example.test/v1");
    assert_eq!(parsed["array"], json!([1]));
    assert!(crate::provider_json::parse_document("{/* unfinished").is_err());
    assert_eq!(
        split_model_reference("gateway/org/model")?,
        ("gateway", "org/model")
    );
    assert!(split_model_reference("/model").is_err());
    assert!(split_model_reference("provider/").is_err());
    Ok(())
}

#[test]
fn legacy_configuration_loads_unchanged_without_provider_documents() -> anyhow::Result<()> {
    let temp = tempdir()?;
    fs::create_dir(temp.path().join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home"))?;
    fs::write(
        &manager.path,
        r#"{"provider":"ollama","model":"org/model"}"#,
    )?;
    let config = manager.load_for_project_with_env(temp.path(), &|_| None)?;
    assert_eq!(config.provider, "ollama");
    assert_eq!(config.model.as_deref(), Some("org/model"));
    assert!(config.provider_baseline.is_none());
    Ok(())
}

#[test]
fn stored_credentials_and_recent_model_survive_restart_without_changing_documents()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    fs::create_dir(temp.path().join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home"))?;
    let source = r#"{"provider":{"groq":{"models":{"one":{},"two":{}}}}}"#;
    fs::write(temp.path().join("rara.json"), source)?;
    manager.save_registry_api_key(
        "groq",
        secrecy::SecretString::from("stored-key".to_string()),
    )?;
    let mut config = manager.load_for_project_with_env(temp.path(), &|_| None)?;
    config.select_registry_model("groq", "two")?;
    manager.save(&config)?;
    let loaded = manager.load_for_project_with_env(temp.path(), &|_| None)?;
    assert_eq!(loaded.model.as_deref(), Some("two"));
    assert_eq!(loaded.api_key(), Some("stored-key"));
    assert_eq!(fs::read_to_string(temp.path().join("rara.json"))?, source);
    config.set_provider("ollama");
    config.set_model(Some("local-model".into()));
    manager.save(&config)?;
    let loaded = manager.load_for_project_with_env(temp.path(), &|_| None)?;
    assert_eq!(loaded.provider, "ollama");
    assert_eq!(loaded.model.as_deref(), Some("local-model"));
    assert!(!manager.path.with_file_name("model-selection.json").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(manager.path.with_file_name("provider-auth.json"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    Ok(())
}

#[test]
fn explicit_model_with_missing_credentials_never_silently_runs_mock() -> anyhow::Result<()> {
    let temp = tempdir()?;
    fs::create_dir(temp.path().join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home"))?;
    fs::write(
        temp.path().join("rara.json"),
        r#"{"model":"groq/one","provider":{"groq":{"models":{"one":{}}}}}"#,
    )?;
    assert!(
        manager
            .load_for_project_with_env(temp.path(), &|_| None)
            .is_err()
    );
    Ok(())
}

#[test]
fn independent_credential_writes_preserve_other_providers() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home"))?;
    let paths: Vec<_> = (0..4).map(|_| manager.path.clone()).collect();
    let handles: Vec<_> = paths
        .into_iter()
        .enumerate()
        .map(|(i, path)| {
            std::thread::spawn(move || {
                ConfigManager { path }.save_registry_api_key(
                    &format!("provider-{i}"),
                    secrecy::SecretString::from(format!("key-{i}")),
                )
            })
        })
        .collect();
    for handle in handles {
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("credential writer panicked"))??;
    }
    assert_eq!(manager.load_provider_auth()?.len(), 4);
    Ok(())
}

#[test]
fn cli_selection_and_key_override_an_unavailable_configured_default() -> anyhow::Result<()> {
    let temp = tempdir()?;
    fs::create_dir(temp.path().join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home"))?;
    fs::write(
        temp.path().join("rara.json"),
        r#"{"model":"groq/one","provider":{"groq":{"models":{"one":{}}},"xai":{"models":{"org/two":{}}}}}"#,
    )?;
    let overrides = crate::ProviderSelectionOverrides {
        model: Some("xai/org/two".into()),
        api_key: Some(secrecy::SecretString::from("cli-key".to_string())),
        ..Default::default()
    };
    let config = manager.load_provider_inputs(temp.path(), &|_| None, &overrides)?;
    assert_eq!(config.provider, "xai");
    assert_eq!(config.model.as_deref(), Some("org/two"));
    assert_eq!(config.api_key(), Some("cli-key"));
    manager.save(&config)?;
    assert!(!fs::read_to_string(&manager.path)?.contains("cli-key"));
    let overrides = crate::ProviderSelectionOverrides {
        provider: Some("ollama".into()),
        model: Some("local".into()),
        ..Default::default()
    };
    let config = manager.load_provider_inputs(temp.path(), &|_| None, &overrides)?;
    assert!(config.provider_registry.selected.is_none());
    Ok(())
}

#[test]
fn registry_connections_do_not_inherit_legacy_profile_or_environment_credentials()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    fs::create_dir(temp.path().join(".git"))?;
    let manager = ConfigManager::new_for_rara_home(temp.path().join("home"))?;
    let mut legacy = manager.load()?;
    legacy.set_provider("openai-compatible");
    legacy.set_api_key("legacy-only-key");
    manager.save(&legacy)?;
    fs::write(
        temp.path().join("rara.json"),
        r#"{"model":"openai-compatible/one","provider":{"openai-compatible":{"options":{"baseURL":"http://localhost:9191/api"},"models":{"one":{}}},"kimi":{"options":{"baseURL":"http://localhost:9192/api"},"models":{"two":{}}}}}"#,
    )?;
    let mut config = manager.load_for_project_with_env(temp.path(), &|_| None)?;
    assert!(config.api_key().is_none());
    assert!(config.effective_provider_surface().api_key.value.is_none());
    assert_eq!(
        config.effective_provider_surface().base_url.value,
        Some("http://localhost:9191/api")
    );
    config.select_registry_model("kimi", "two")?;
    config.apply_provider_environment_defaults_from(|_| Some("unrelated-environment-key".into()));
    assert!(config.api_key().is_none());
    Ok(())
}
