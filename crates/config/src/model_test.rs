use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use secrecy::ExposeSecret;
use tempfile::tempdir;

use super::{
    ConfigManager, ContextFileSearchPolicy, NowledgeMemMode, NowledgeMemPluginConfig,
    OpenAiEndpointKind, OpenAiEndpointProfile, ProviderConfigState, RaraConfig,
    workspace_data_dir_for_home,
};
use crate::defaults::{
    DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_CHATGPT_BASE_URL, DEFAULT_CODEX_MODEL,
    DEFAULT_KIMI_BASE_URL, DEFAULT_KIMI_CODING_BASE_URL, DEFAULT_KIMI_CODING_MODEL,
    DEFAULT_KIMI_MODEL, DEFAULT_OPENROUTER_BASE_URL, DEFAULT_OPENROUTER_MODEL,
    DEFAULT_REASONING_SUMMARY, REASONING_SUMMARY_NONE,
};
use crate::provider_surface::ConfigValueSource;

#[test]
fn secret_api_key_roundtrips_through_json() {
    let mut config = RaraConfig {
        provider: "codex".to_string(),
        ..Default::default()
    };
    config.set_api_key("sk-test-value");

    let json = serde_json::to_string(&config).expect("serialize config");
    let restored: RaraConfig = serde_json::from_str(&json).expect("deserialize config");

    assert_eq!(restored.api_key(), Some("sk-test-value"));
    assert!(restored.has_api_key());
}

#[test]
fn setting_inactive_provider_api_key_preserves_active_provider_credentials() {
    let mut config = RaraConfig {
        provider: "codex".to_string(),
        ..Default::default()
    };
    config.set_api_key("sk-codex");

    config.set_provider_api_key("kimi", "sk-kimi");

    assert_eq!(config.provider, "codex");
    assert_eq!(config.api_key(), Some("sk-codex"));
    let kimi_profile = config
        .openai_profiles
        .get(OpenAiEndpointKind::Kimi.default_profile_id())
        .expect("Kimi profile");
    assert_eq!(
        kimi_profile
            .api_key
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("sk-kimi")
    );
    assert_eq!(
        kimi_profile.base_url.as_deref(),
        Some(DEFAULT_KIMI_BASE_URL)
    );
    assert_eq!(kimi_profile.model.as_deref(), Some(DEFAULT_KIMI_MODEL));
}

#[test]
fn setting_inactive_kimi_coding_key_uses_the_coding_profile() {
    let mut config = RaraConfig {
        provider: "codex".to_string(),
        ..Default::default()
    };
    config.set_api_key("sk-codex");

    config.set_provider_api_key("kimi-coding", "sk-kimi-code");

    assert_eq!(config.provider, "codex");
    assert_eq!(config.api_key(), Some("sk-codex"));
    let profile = config
        .openai_profiles
        .get(OpenAiEndpointKind::KimiCoding.default_profile_id())
        .expect("Kimi For Coding profile");
    assert_eq!(
        profile.api_key.as_ref().map(ExposeSecret::expose_secret),
        Some("sk-kimi-code")
    );
    assert_eq!(
        profile.base_url.as_deref(),
        Some(DEFAULT_KIMI_CODING_BASE_URL)
    );
    assert_eq!(profile.model.as_deref(), Some(DEFAULT_KIMI_CODING_MODEL));
}

#[test]
fn empty_secret_is_not_counted_as_configured() {
    let mut config = RaraConfig::default();
    config.set_api_key("");
    assert!(!config.has_api_key());
}

#[test]
fn loads_codex_style_sandbox_workspace_network_access() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "codex",
            "sandbox_workspace_write": {
                "network_access": true
            }
        }"#,
    )
    .expect("deserialize config");

    assert!(config.sandbox_workspace_write.network_access);
}

#[test]
fn sandbox_workspace_network_access_defaults_on() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "codex"
        }"#,
    )
    .expect("deserialize config");

    assert!(config.sandbox_workspace_write.network_access);
}

#[test]
fn sandbox_workspace_network_access_empty_object_defaults_on() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "codex",
            "sandbox_workspace_write": {}
        }"#,
    )
    .expect("deserialize config");

    assert!(config.sandbox_workspace_write.network_access);
}

#[test]
fn context_file_search_defaults_to_paths_only_and_is_omitted() {
    let config = RaraConfig::default();

    assert_eq!(
        config.context_file_search,
        ContextFileSearchPolicy::PathsOnly
    );

    let json = serde_json::to_string(&config).expect("serialize config");
    assert!(!json.contains("context_file_search"));
}

#[test]
fn context_file_search_can_disable_automatic_retrieval_candidates() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "codex",
            "context_file_search": "off"
        }"#,
    )
    .expect("deserialize config");

    assert_eq!(config.context_file_search, ContextFileSearchPolicy::Off);
}

#[test]
fn plugin_dirs_default_empty_and_omitted() {
    let config = RaraConfig::default();

    assert!(config.plugin_dirs.is_empty());

    let json = serde_json::to_string(&config).expect("serialize config");
    assert!(!json.contains("plugin_dirs"));
}

#[test]
fn plugin_dirs_can_be_loaded_from_config() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "deepseek",
            "plugin_dirs": ["plugins-a", "/tmp/plugins-b"]
        }"#,
    )
    .expect("deserialize config");

    assert_eq!(
        config.plugin_dirs,
        vec![PathBuf::from("plugins-a"), PathBuf::from("/tmp/plugins-b")]
    );
}

#[test]
fn builtin_plugins_default_enabled_and_omitted() {
    let config = RaraConfig::default();

    assert!(config.builtin_plugins.nowledge_mem.enabled);
    assert_eq!(
        config.builtin_plugins.nowledge_mem.url,
        "http://127.0.0.1:14242/mcp/"
    );

    let json = serde_json::to_string(&config).expect("serialize config");
    assert!(!json.contains("builtin_plugins"));
}

#[test]
fn builtin_nowledge_mem_config_can_override_endpoint_and_headers() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "deepseek",
            "builtin_plugins": {
                "nowledge_mem": {
                    "enabled": false,
                    "url": "http://localhost:24242/mcp/",
                    "http_headers": {
                        "APP": "CustomRara",
                        "X-NMEM-Space": "workspace"
                    }
                }
            }
        }"#,
    )
    .expect("deserialize config");

    assert!(!config.builtin_plugins.nowledge_mem.enabled);
    assert_eq!(
        config.builtin_plugins.nowledge_mem.url,
        "http://localhost:24242/mcp/"
    );
    assert_eq!(
        config.builtin_plugins.nowledge_mem.http_headers,
        BTreeMap::from([
            ("APP".to_string(), "CustomRara".to_string()),
            ("X-NMEM-Space".to_string(), "workspace".to_string())
        ])
    );
}

#[test]
fn builtin_nowledge_mem_cloud_mode_derives_remote_mcp_and_env_headers() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "deepseek",
            "builtin_plugins": {
                "nowledge_mem": {
                    "mode": "cloud",
                    "url": "https://cloud.nowledge.co",
                    "api_key_env_var": "RARA_NMEM_API_KEY",
                    "space_id_env_var": "RARA_NMEM_SPACE"
                }
            }
        }"#,
    )
    .expect("deserialize config");

    let mem = &config.builtin_plugins.nowledge_mem;
    assert_eq!(mem.mode, NowledgeMemMode::Cloud);
    assert_eq!(mem.mcp_url(), "https://cloud.nowledge.co/remote-api/mcp/");
    assert_eq!(
        mem.env_http_headers(),
        Some(BTreeMap::from([
            ("Authorization".to_string(), "RARA_NMEM_API_KEY".to_string()),
            (
                "X-NMEM-API-Key".to_string(),
                "RARA_NMEM_API_KEY".to_string()
            ),
            ("X-Nmem-Space-Id".to_string(), "RARA_NMEM_SPACE".to_string())
        ]))
    );
}

#[test]
fn builtin_nowledge_mem_cloud_mode_defaults_to_nowledge_cloud() {
    let mem = NowledgeMemPluginConfig {
        mode: NowledgeMemMode::Cloud,
        ..Default::default()
    };

    assert_eq!(mem.mcp_url(), "https://cloud.nowledge.co/remote-api/mcp/");
    assert_eq!(mem.api_url(), "https://cloud.nowledge.co/remote-api");
}

#[test]
fn builtin_nowledge_mem_api_key_roundtrips_through_config() {
    let mut config = RaraConfig::default();
    config
        .builtin_plugins
        .nowledge_mem
        .set_api_key("nmem_test_key");

    let serialized = serde_json::to_string(&config).expect("serialize config");
    let restored: RaraConfig = serde_json::from_str(&serialized).expect("restore config");

    assert_eq!(
        restored.builtin_plugins.nowledge_mem.api_key(),
        Some("nmem_test_key")
    );
}

#[test]
fn sandbox_workspace_network_access_can_be_disabled() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "codex",
            "sandbox_workspace_write": {
                "network_access": false
            }
        }"#,
    )
    .expect("deserialize config");

    assert!(!config.sandbox_workspace_write.network_access);
    let json = serde_json::to_string(&config).expect("serialize config");
    assert!(json.contains("sandbox_workspace_write"));
}

#[test]
fn loads_tui_theme_config() {
    let config: RaraConfig = serde_json::from_str(
        r##"{
            "provider": "codex",
            "tui": {
                "theme": {
                    "name": "nord",
                    "syntax_theme": "Nord",
                    "tokens": {
                        "text.accent": "#88c0d0",
                        "picker.highlight.bg": "ansi:12"
                    }
                }
            }
        }"##,
    )
    .expect("deserialize config");

    assert_eq!(config.tui.theme.name, "nord");
    assert_eq!(config.tui.theme.syntax_theme.as_deref(), Some("Nord"));
    assert_eq!(
        config
            .tui
            .theme
            .tokens
            .get("text.accent")
            .map(String::as_str),
        Some("#88c0d0")
    );
    assert_eq!(
        config
            .tui
            .theme
            .tokens
            .get("picker.highlight.bg")
            .map(String::as_str),
        Some("ansi:12")
    );

    let json = serde_json::to_string(&config).expect("serialize config");
    assert!(json.contains("\"tui\""));
}

#[test]
fn auxiliary_model_reads_legacy_utility_model_key() {
    let config: RaraConfig = serde_json::from_str(
        r#"{
            "provider": "deepseek",
            "utility_model": "deepseek-v4-lite",
            "provider_states": {
                "codex": {
                    "utility_model": "gpt-5.4-mini"
                }
            },
            "openai_profiles": {
                "deepseek-default": {
                    "id": "deepseek-default",
                    "label": "DeepSeek",
                    "kind": "deepseek",
                    "utility_model": "deepseek-v4-lite"
                }
            }
        }"#,
    )
    .expect("deserialize config");

    assert_eq!(config.auxiliary_model.as_deref(), Some("deepseek-v4-lite"));
    assert_eq!(
        config
            .provider_states
            .get("codex")
            .and_then(|state| state.auxiliary_model.as_deref()),
        Some("gpt-5.4-mini")
    );
    assert_eq!(
        config
            .openai_profiles
            .get("deepseek-default")
            .and_then(|profile| profile.auxiliary_model.as_deref()),
        Some("deepseek-v4-lite")
    );
    let json = serde_json::to_string(&config).expect("serialize config");
    assert!(json.contains("auxiliary_model"));
    assert!(!json.contains("utility_model"));
}

#[test]
fn provider_switch_restores_provider_specific_settings() {
    let mut config = RaraConfig {
        provider: "codex".to_string(),
        ..Default::default()
    };
    config.set_api_key("sk-codex");
    config.set_model(Some("codex".to_string()));
    config.set_auxiliary_model(Some("gpt-5.4-mini".to_string()));
    config.set_reasoning_effort(Some("high".to_string()));
    config.set_reasoning_summary(Some("detailed".to_string()));
    config.set_base_url(Some("http://localhost:8080".to_string()));

    config.set_provider("ollama");
    assert_eq!(config.provider, "ollama");
    assert!(config.api_key().is_none());
    assert!(config.model.is_none());
    assert!(config.base_url.is_none());

    config.set_model(Some("qwen3".to_string()));
    config.set_base_url(Some("http://localhost:11434".to_string()));
    config.set_num_ctx(Some(32768));

    config.set_provider("codex");
    assert_eq!(config.api_key(), Some("sk-codex"));
    assert_eq!(config.model.as_deref(), Some("codex"));
    assert_eq!(config.auxiliary_model.as_deref(), Some("gpt-5.4-mini"));
    assert_eq!(config.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(config.reasoning_summary.as_deref(), Some("detailed"));
    assert_eq!(config.base_url.as_deref(), Some("http://localhost:8080"));
    assert_eq!(config.num_ctx, None);

    config.set_provider("ollama");
    assert_eq!(config.model.as_deref(), Some("qwen3"));
    assert!(config.auxiliary_model.is_none());
    assert_eq!(config.reasoning_effort, None);
    assert_eq!(
        config.reasoning_summary.as_deref(),
        Some(DEFAULT_REASONING_SUMMARY)
    );
    assert_eq!(config.base_url.as_deref(), Some("http://localhost:11434"));
    assert_eq!(config.num_ctx, Some(32768));
}

#[test]
fn migrate_legacy_openai_provider_into_active_profile() {
    let mut config = RaraConfig {
        provider: "kimi".to_string(),
        base_url: Some("https://api.moonshot.cn/v1".to_string()),
        model: Some("kimi-k2".to_string()),
        reasoning_summary: Some("detailed".to_string()),
        ..Default::default()
    };
    config.set_api_key("sk-kimi");

    config.migrate_legacy_provider_state();

    assert_eq!(config.provider, "openai-compatible");
    assert_eq!(config.active_openai_profile_id(), Some("kimi-default"));
    assert_eq!(
        config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Kimi)
    );
    let profile = config
        .active_openai_profile()
        .expect("active openai profile");
    assert_eq!(profile.label, "Moonshot AI");
    assert_eq!(
        profile.api_key.as_ref().map(|v| v.expose_secret()),
        Some("sk-kimi")
    );
    assert_eq!(
        profile.base_url.as_deref(),
        Some("https://api.moonshot.cn/v1")
    );
    assert_eq!(profile.model.as_deref(), Some("kimi-k2"));
    assert_eq!(config.model.as_deref(), Some("kimi-k2"));
}

#[test]
fn kimi_profile_uses_current_documented_defaults() {
    let mut config = RaraConfig::default();

    config.select_openai_profile(
        OpenAiEndpointKind::Kimi.default_profile_id(),
        OpenAiEndpointKind::Kimi.label(),
        OpenAiEndpointKind::Kimi,
    );

    assert_eq!(config.provider, "openai-compatible");
    assert_eq!(
        config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Kimi)
    );
    assert_eq!(config.base_url.as_deref(), Some(DEFAULT_KIMI_BASE_URL));
    assert_eq!(config.model.as_deref(), Some(DEFAULT_KIMI_MODEL));
}

#[test]
fn kimi_coding_profile_uses_the_dedicated_coding_endpoint() {
    let mut config = RaraConfig::default();

    config.select_openai_profile(
        OpenAiEndpointKind::KimiCoding.default_profile_id(),
        OpenAiEndpointKind::KimiCoding.label(),
        OpenAiEndpointKind::KimiCoding,
    );

    assert_eq!(config.provider, "openai-compatible");
    assert_eq!(
        config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::KimiCoding)
    );
    assert_eq!(
        config.base_url.as_deref(),
        Some(DEFAULT_KIMI_CODING_BASE_URL)
    );
    assert_eq!(config.model.as_deref(), Some(DEFAULT_KIMI_CODING_MODEL));
}

#[test]
fn kimi_profile_can_use_moonshot_api_key_from_environment_without_persisting_it() {
    let mut config = RaraConfig::default();
    config.select_openai_profile(
        OpenAiEndpointKind::Kimi.default_profile_id(),
        OpenAiEndpointKind::Kimi.label(),
        OpenAiEndpointKind::Kimi,
    );

    config.apply_provider_environment_defaults_from(|key| {
        (key == "MOONSHOT_API_KEY").then(|| "sk-moonshot".to_string())
    });

    assert_eq!(config.api_key(), Some("sk-moonshot"));
    assert_eq!(
        config.effective_provider_surface().api_key.source,
        ConfigValueSource::Environment
    );
    let json = serde_json::to_string(&config).expect("serialize config");
    assert!(!json.contains("sk-moonshot"));
    assert!(!json.contains("runtime_api_key"));
}

#[test]
fn moonshot_profile_does_not_use_kimi_code_environment_key() {
    let mut config = RaraConfig::default();
    config.select_openai_profile(
        OpenAiEndpointKind::Kimi.default_profile_id(),
        OpenAiEndpointKind::Kimi.label(),
        OpenAiEndpointKind::Kimi,
    );

    config.apply_provider_environment_defaults_from(|key| {
        (key == "KIMI_API_KEY").then(|| "sk-kimi".to_string())
    });

    assert_eq!(config.api_key(), None);
    assert_eq!(
        config.effective_provider_surface().api_key.source,
        ConfigValueSource::Unset
    );
}

#[test]
fn kimi_coding_profile_uses_kimi_api_key_without_persisting_it() {
    let mut config = RaraConfig::default();
    config.select_openai_profile(
        OpenAiEndpointKind::KimiCoding.default_profile_id(),
        OpenAiEndpointKind::KimiCoding.label(),
        OpenAiEndpointKind::KimiCoding,
    );

    config.apply_provider_environment_defaults_from(|key| {
        (key == "KIMI_API_KEY").then(|| "sk-kimi".to_string())
    });

    assert_eq!(config.api_key(), Some("sk-kimi"));
    assert_eq!(
        config.effective_provider_surface().api_key.source,
        ConfigValueSource::Environment
    );
    let json = serde_json::to_string(&config).expect("serialize config");
    assert!(!json.contains("sk-kimi"));
    assert!(!json.contains("runtime_api_key"));
}

#[test]
fn explicit_kimi_api_key_overrides_environment_default() {
    let mut config = RaraConfig::default();
    config.select_openai_profile(
        OpenAiEndpointKind::Kimi.default_profile_id(),
        OpenAiEndpointKind::Kimi.label(),
        OpenAiEndpointKind::Kimi,
    );
    config.set_api_key("sk-explicit");

    config.apply_provider_environment_defaults_from(|key| {
        (key == "MOONSHOT_API_KEY").then(|| "sk-moonshot".to_string())
    });

    assert_eq!(config.api_key(), Some("sk-explicit"));
    assert_eq!(
        config.effective_provider_surface().api_key.source,
        ConfigValueSource::ProviderState
    );
}

#[test]
fn provider_state_migration_preserves_multiple_openai_profiles() {
    let mut config = RaraConfig {
        provider: "openrouter".to_string(),
        provider_states: BTreeMap::from([
            (
                "kimi".to_string(),
                ProviderConfigState {
                    api_key: Some("sk-kimi".into()),
                    base_url: Some(DEFAULT_KIMI_BASE_URL.to_string()),
                    model: Some(DEFAULT_KIMI_MODEL.to_string()),
                    ..Default::default()
                },
            ),
            (
                "openrouter".to_string(),
                ProviderConfigState {
                    api_key: Some("sk-openrouter".into()),
                    base_url: Some(DEFAULT_OPENROUTER_BASE_URL.to_string()),
                    model: Some(DEFAULT_OPENROUTER_MODEL.to_string()),
                    ..Default::default()
                },
            ),
        ]),
        ..Default::default()
    };

    config.migrate_legacy_provider_state();

    assert_eq!(config.provider, "openai-compatible");
    assert_eq!(
        config.active_openai_profile_id(),
        Some("openrouter-default")
    );
    assert!(config.openai_profiles.contains_key("kimi-default"));
    assert!(config.openai_profiles.contains_key("openrouter-default"));
    assert!(config.provider_states.is_empty());
}

#[test]
fn provider_state_migration_does_not_switch_unrelated_provider() {
    let mut config = RaraConfig {
        provider: "ollama".to_string(),
        model: Some("qwen3".to_string()),
        provider_states: BTreeMap::from([(
            "openrouter".to_string(),
            ProviderConfigState {
                api_key: Some("sk-openrouter".into()),
                base_url: Some(DEFAULT_OPENROUTER_BASE_URL.to_string()),
                model: Some(DEFAULT_OPENROUTER_MODEL.to_string()),
                ..Default::default()
            },
        )]),
        ..Default::default()
    };

    config.migrate_legacy_provider_state();

    assert_eq!(config.provider, "ollama");
    assert_eq!(config.model.as_deref(), Some("qwen3"));
    assert!(config.openai_profiles.contains_key("openrouter-default"));
}

#[test]
fn provider_state_migration_preserves_existing_openai_active_profile_id() {
    let mut config = RaraConfig {
        provider: "openai-compatible".to_string(),
        active_openai_profile_id: Some("openrouter-main".to_string()),
        openai_profiles: BTreeMap::from([(
            "openrouter-main".to_string(),
            OpenAiEndpointProfile {
                id: "openrouter-main".to_string(),
                label: "OpenRouter main".to_string(),
                kind: OpenAiEndpointKind::Openrouter,
                ..Default::default()
            },
        )]),
        base_url: Some("https://openrouter.ai/api/v1".to_string()),
        model: Some("anthropic/claude-sonnet-4".to_string()),
        api_key: Some("sk-openrouter-main".into()),
        ..Default::default()
    };

    config.migrate_legacy_provider_state();

    assert_eq!(config.provider, "openai-compatible");
    assert_eq!(config.active_openai_profile_id(), Some("openrouter-main"));
    let profile = config
        .active_openai_profile()
        .expect("active openai profile");
    assert_eq!(profile.id, "openrouter-main");
    assert_eq!(profile.label, "OpenRouter main");
    assert_eq!(profile.kind, OpenAiEndpointKind::Openrouter);
    assert_eq!(profile.model.as_deref(), Some("anthropic/claude-sonnet-4"));
    assert_eq!(
        profile.api_key.as_ref().map(|value| value.expose_secret()),
        Some("sk-openrouter-main")
    );
}

#[test]
fn switching_openai_profiles_restores_profile_specific_fields() {
    let mut config = RaraConfig::default();

    config.select_openai_profile(
        "openrouter-main",
        "OpenRouter main",
        OpenAiEndpointKind::Openrouter,
    );
    config.set_api_key("sk-openrouter-main");
    config.set_base_url(Some("https://openrouter.ai/api/v1".to_string()));
    config.set_model(Some("anthropic/claude-sonnet-4".to_string()));

    config.select_openai_profile(
        "openrouter-backup",
        "OpenRouter backup",
        OpenAiEndpointKind::Openrouter,
    );
    config.set_api_key("sk-openrouter-backup");
    config.set_model(Some("openai/gpt-4o-mini".to_string()));

    config.select_openai_profile(
        "openrouter-main",
        "OpenRouter main",
        OpenAiEndpointKind::Openrouter,
    );

    assert_eq!(config.provider, "openai-compatible");
    assert_eq!(config.active_openai_profile_id(), Some("openrouter-main"));
    assert_eq!(config.api_key(), Some("sk-openrouter-main"));
    assert_eq!(config.model.as_deref(), Some("anthropic/claude-sonnet-4"));
}

#[test]
fn switching_away_and_back_to_openai_compatible_keeps_active_profile() {
    let mut config = RaraConfig::default();

    config.select_openai_profile(
        "openrouter-main",
        "OpenRouter main",
        OpenAiEndpointKind::Openrouter,
    );
    config.set_api_key("sk-openrouter-main");
    config.set_model(Some("anthropic/claude-sonnet-4".to_string()));

    config.set_provider("codex");
    config.set_model(Some("gpt-5.1-codex".to_string()));
    config.set_provider("ollama");
    config.set_model(Some("gemma4".to_string()));
    config.set_provider("openai-compatible");

    assert_eq!(config.active_openai_profile_id(), Some("openrouter-main"));
    assert_eq!(config.api_key(), Some("sk-openrouter-main"));
    assert_eq!(config.model.as_deref(), Some("anthropic/claude-sonnet-4"));
}

#[test]
fn load_migrates_legacy_thinking_to_reasoning_summary() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("config.json");
    fs::write(
        &path,
        r#"{
  "provider": "codex",
  "thinking": false,
  "provider_states": {
"codex": {
  "thinking": true
}
  }
}"#,
    )
    .expect("write config");
    let manager = ConfigManager { path };

    let config = manager.load().expect("load config");

    assert_eq!(
        config.reasoning_summary.as_deref(),
        Some(REASONING_SUMMARY_NONE)
    );
    assert_eq!(
        config.provider_states["codex"].reasoning_summary.as_deref(),
        Some(DEFAULT_REASONING_SUMMARY)
    );
}

#[path = "model_test/persistence.rs"]
mod persistence;
