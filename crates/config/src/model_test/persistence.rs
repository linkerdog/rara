use super::*;

#[test]
fn saves_and_loads_allowed_command_prefix_rules() {
    let dir = tempdir().expect("tempdir");
    let manager =
        ConfigManager::new_for_rara_home(dir.path().join(".rara")).expect("config manager");

    manager
        .save_allowed_command_prefixes(&[
            "git push".to_string(),
            "cargo test".to_string(),
            "git push".to_string(),
        ])
        .expect("save rules");
    let loaded = manager.load_allowed_command_prefixes().expect("load rules");

    assert_eq!(
        loaded,
        vec!["cargo test".to_string(), "git push".to_string()]
    );
    assert_eq!(
        fs::read_to_string(manager.rules_path()).expect("read rules"),
        "prefix_rule(pattern=[\"git\", \"push\"], decision=\"allow\")\n\
prefix_rule(pattern=[\"cargo\", \"test\"], decision=\"allow\")\n"
    );
}

#[test]
fn loads_codex_style_allowed_command_prefix_rules() {
    let dir = tempdir().expect("tempdir");
    let manager =
        ConfigManager::new_for_rara_home(dir.path().join(".rara")).expect("config manager");
    fs::create_dir_all(manager.rules_path().parent().unwrap()).expect("create rules dir");
    fs::write(
        manager.rules_path(),
        r#"
prefix_rule(
pattern=["git", "push"],
)
prefix_rule(pattern=["cargo","test"], decision="allow")
prefix_rule(pattern=["rm", "-rf"], decision="prompt")
"#,
    )
    .expect("write rules");

    assert_eq!(
        manager.load_allowed_command_prefixes().expect("load rules"),
        vec!["cargo test".to_string(), "git push".to_string()]
    );
}

#[test]
fn invalid_reasoning_summary_normalizes_to_auto() {
    let mut config = RaraConfig::default();
    config.set_reasoning_summary(Some("verbose".to_string()));

    assert_eq!(
        config.reasoning_summary.as_deref(),
        Some(DEFAULT_REASONING_SUMMARY)
    );
}

#[test]
fn apply_codex_defaults_migrates_legacy_model_and_base_url() {
    let mut config = RaraConfig {
        provider: "codex".to_string(),
        ..Default::default()
    };
    config.set_model(Some("codex".to_string()));
    config.set_base_url(Some("http://localhost:8080".to_string()));

    config.apply_codex_defaults();

    assert_eq!(config.model.as_deref(), Some(DEFAULT_CODEX_MODEL));
    assert_eq!(config.base_url.as_deref(), Some(DEFAULT_CODEX_BASE_URL));
}

#[test]
fn apply_codex_defaults_for_base_url_switches_between_known_codex_defaults() {
    let mut config = RaraConfig {
        provider: "codex".to_string(),
        base_url: Some(DEFAULT_CODEX_BASE_URL.to_string()),
        model: Some(DEFAULT_CODEX_MODEL.to_string()),
        ..Default::default()
    };

    config.apply_codex_defaults_for_base_url(DEFAULT_CODEX_CHATGPT_BASE_URL);

    assert_eq!(
        config.base_url.as_deref(),
        Some(DEFAULT_CODEX_CHATGPT_BASE_URL)
    );
    assert_eq!(config.model.as_deref(), Some(DEFAULT_CODEX_MODEL));
}

#[test]
fn config_manager_uses_rara_home() {
    let dir = tempdir().expect("tempdir");
    let manager =
        ConfigManager::new_for_rara_home(dir.path().join(".rara")).expect("config manager");
    assert_eq!(manager.path, dir.path().join(".rara").join("config.json"));
}

#[test]
fn config_toml_path_lives_next_to_config_json() {
    let dir = tempdir().expect("tempdir");
    let manager =
        ConfigManager::new_for_rara_home(dir.path().join(".rara")).expect("config manager");

    assert_eq!(
        manager.config_toml_path(),
        dir.path().join(".rara").join("config.toml")
    );
}

#[test]
fn load_mcp_registry_for_project_returns_empty_when_configs_are_missing() {
    let dir = tempdir().expect("tempdir");
    let manager =
        ConfigManager::new_for_rara_home(dir.path().join(".rara")).expect("config manager");
    let project = dir.path().join("project");
    fs::create_dir_all(&project).expect("project dir");

    let registry = manager
        .load_mcp_registry_for_project(&project)
        .expect("mcp registry");

    assert!(registry.servers.is_empty());
}

#[test]
fn workspace_data_dir_lives_under_global_rara_home() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("repo");
    fs::create_dir_all(&root).expect("mkdir root");

    let rara_home = temp.path().join(".rara-home");
    let data_dir = workspace_data_dir_for_home(&root, &rara_home).expect("workspace data dir");

    assert!(data_dir.starts_with(rara_home.join("workspaces")));
    assert!(data_dir.exists());
}

#[test]
fn load_returns_error_for_invalid_json() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("config.json");
    fs::write(&path, "{invalid json").expect("write invalid config");
    let manager = ConfigManager { path };

    let err = manager.load().expect_err("invalid config should fail");
    assert!(err.to_string().contains("failed to parse"));
}
