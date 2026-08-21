use std::path::PathBuf;
use std::sync::Arc;

use tempfile::tempdir;

use super::{
    ConfigSubagentBackendResolver, RuntimeBootstrapOptions, append_builtin_prompt_instructions,
    append_multi_agent_prompt_instructions, build_backend_with_progress,
    build_backend_with_progress_for_home, initialize_rara_context,
    initialize_rara_context_for_workspace_with_options, memory_handle_uri_for_workspace,
    ollama_thinking_enabled,
};
use crate::config::{
    DEFAULT_REASONING_SUMMARY, MultiAgentPolicy, ProviderConfigState, REASONING_SUMMARY_NONE,
    RaraConfig,
};
use crate::llm::{LlmBackend, MockLlm};
use crate::tools::agent::{AgentTreeControl, SubagentBackendResolver, SubagentProviderTarget};
use crate::workspace::WorkspaceMemory;

#[test]
fn nowledge_mem_guidance_is_injected_into_the_default_prompt() {
    let mut prompt = crate::prompt::PromptRuntimeConfig::default();
    append_builtin_prompt_instructions(&mut prompt, &crate::config::BuiltinPluginConfig::default());

    let instructions = prompt
        .append_system_prompt
        .expect("enabled builtin memory should add prompt guidance");
    assert!(instructions.contains("Context Bundle"));
    assert!(instructions.contains("After context compaction"));
}

#[test]
fn proactive_multi_agent_prompt_is_read_only_and_effort_independent() {
    let mut prompt = crate::prompt::PromptRuntimeConfig::default();
    append_multi_agent_prompt_instructions(&mut prompt, MultiAgentPolicy::ProactiveReadOnly);

    let instructions = prompt.append_system_prompt.expect("policy prompt");
    assert!(instructions.contains("proactively delegate"));
    assert!(instructions.contains("read-only roles"));
    assert!(instructions.contains("Model selection and permissions are independent"));
    assert!(!instructions.contains("reasoning_effort"));
}

#[tokio::test]
async fn disabled_multi_agent_policy_removes_orchestration_tools() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let config = RaraConfig {
        provider: "mock".to_string(),
        multi_agent_policy: MultiAgentPolicy::Disabled,
        ..Default::default()
    };
    let options =
        RuntimeBootstrapOptions::default().with_rara_home(Some(temp.path().join("state")));

    let bootstrap = initialize_rara_context_for_workspace_with_options(
        &config,
        Some(&workspace),
        None,
        options,
    )
    .await
    .expect("bootstrap");
    let names = bootstrap
        .tool_manager
        .get_schemas()
        .into_iter()
        .filter_map(|schema| schema["name"].as_str().map(str::to_string))
        .collect::<Vec<_>>();

    assert!(!names.iter().any(|name| name == "spawn_agent"));
    assert!(!names.iter().any(|name| name == "wait_agent"));
    assert!(names.iter().any(|name| name == "read_file"));
}

#[tokio::test]
async fn bootstrap_reuses_supplied_agent_tree_control() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let config = RaraConfig {
        provider: "mock".to_string(),
        ..Default::default()
    };
    let control = Arc::new(AgentTreeControl::default());
    let options = RuntimeBootstrapOptions::default()
        .with_rara_home(Some(temp.path().join("state")))
        .with_agent_tree_control(Some(control.clone()));

    let bootstrap = initialize_rara_context_for_workspace_with_options(
        &config,
        Some(&workspace),
        None,
        options,
    )
    .await
    .expect("bootstrap");

    assert!(Arc::ptr_eq(&bootstrap.agent_tree_control, &control));
    let agent = bootstrap.into_agent().await;
    assert!(Arc::ptr_eq(
        &agent.agent_tree_control().expect("agent tree control"),
        &control
    ));
}

#[tokio::test]
async fn explicit_state_root_scopes_provider_auth_storage() {
    let temp = tempdir().expect("tempdir");
    let state_root = temp.path().join("state");
    let config = RaraConfig {
        provider: "gemini-code-assist".to_string(),
        model: Some("gemini-2.5-flash".to_string()),
        ..Default::default()
    };

    let _backend = build_backend_with_progress_for_home(&config, None, Some(&state_root))
        .await
        .expect("backend");

    assert!(state_root.join("auth").is_dir());
}

#[test]
fn memory_handle_uri_is_workspace_scoped() {
    let temp = tempdir().expect("tempdir");
    let workspace =
        WorkspaceMemory::from_paths(temp.path().join("repo"), temp.path().join(".rara"));

    assert_eq!(
        memory_handle_uri_for_workspace(&workspace),
        temp.path()
            .join(".rara")
            .join("memory")
            .display()
            .to_string()
    );
}

#[tokio::test]
async fn initialize_rara_context_surfaces_prompt_runtime_warnings() {
    let config = RaraConfig {
        provider: "mock".into(),
        system_prompt_file: Some("missing-system-prompt.md".into()),
        ..Default::default()
    };

    let bootstrap = initialize_rara_context(&config, None)
        .await
        .expect("bootstrap");

    assert!(
        bootstrap
            .warnings
            .iter()
            .any(|warning| warning.contains("system prompt"))
    );
}

#[tokio::test]
async fn initialize_rara_context_registers_mcp_tool_search() {
    let config = RaraConfig {
        provider: "mock".into(),
        ..Default::default()
    };

    let bootstrap = initialize_rara_context(&config, None)
        .await
        .expect("bootstrap");
    let schemas = bootstrap.tool_manager.get_schemas();
    let names = schemas
        .iter()
        .filter_map(|schema| schema["name"].as_str())
        .collect::<Vec<_>>();

    assert!(names.contains(&"mcp_tool_search"));
}

#[tokio::test]
async fn unsupported_provider_returns_error() {
    let config = RaraConfig {
        provider: "does-not-exist".to_string(),
        ..Default::default()
    };

    let err = match build_backend_with_progress(&config, None).await {
        Ok(_) => panic!("unsupported provider should fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("Unsupported provider 'does-not-exist'")
    );
}

#[tokio::test]
async fn ollama_requires_explicit_model_selection() {
    let config = RaraConfig {
        provider: "ollama".to_string(),
        model: None,
        ..Default::default()
    };

    let err = match build_backend_with_progress(&config, None).await {
        Ok(_) => panic!("ollama without model should fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("Model required for Ollama provider")
    );
}

#[tokio::test]
async fn ollama_openai_requires_explicit_model_selection() {
    let config = RaraConfig {
        provider: "ollama-openai".to_string(),
        model: None,
        ..Default::default()
    };

    let err = match build_backend_with_progress(&config, None).await {
        Ok(_) => panic!("ollama-openai without model should fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("Model required for Ollama OpenAI provider")
    );
}

#[test]
fn ollama_thinking_respects_reasoning_summary_none() {
    let config = RaraConfig {
        provider: "ollama".into(),
        thinking: Some(true),
        reasoning_summary: Some(REASONING_SUMMARY_NONE.to_string()),
        ..Default::default()
    };

    assert!(!ollama_thinking_enabled(&config));
}

#[test]
fn ollama_thinking_defaults_on_for_auto_reasoning_summary() {
    let config = RaraConfig {
        provider: "ollama".into(),
        thinking: None,
        reasoning_summary: Some(DEFAULT_REASONING_SUMMARY.to_string()),
        ..Default::default()
    };

    assert!(ollama_thinking_enabled(&config));
}

#[test]
fn runtime_bootstrap_options_preserve_plugin_dirs() {
    let plugin_dirs = vec![
        PathBuf::from("/tmp/rara-plugin-a"),
        PathBuf::from("plugins-b"),
    ];
    let options = RuntimeBootstrapOptions::with_plugin_dirs(plugin_dirs.clone());
    assert_eq!(options.plugin_dirs, plugin_dirs);
}

#[tokio::test]
async fn config_subagent_backend_resolver_builds_target_backend() {
    let config = Arc::new(RaraConfig {
        provider: "codex".to_string(),
        model: Some("gpt-5.1-codex".to_string()),
        ..Default::default()
    });
    let resolver = ConfigSubagentBackendResolver::new(config);
    let inherited: Arc<dyn LlmBackend> = Arc::new(MockLlm);

    let resolved = resolver
        .resolve_backend(
            Some(&SubagentProviderTarget {
                provider: Some("mock".to_string()),
                model: Some("mock-worker".to_string()),
            }),
            inherited,
        )
        .await
        .expect("resolved backend");

    assert_eq!(resolved.provider, "mock");
    assert_eq!(resolved.model, "mock-worker");
}

#[tokio::test]
async fn config_subagent_backend_resolver_builds_configured_provider_state() {
    let mut config = RaraConfig {
        provider: "codex".to_string(),
        model: Some("gpt-5.1-codex".to_string()),
        ..Default::default()
    };
    config.provider_states.insert(
        "groq-fast".to_string(),
        ProviderConfigState {
            base_url: Some("https://api.groq.com/openai/v1".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            ..Default::default()
        },
    );
    let resolver = ConfigSubagentBackendResolver::new(Arc::new(config));
    let inherited: Arc<dyn LlmBackend> = Arc::new(MockLlm);

    let resolved = resolver
        .resolve_backend(
            Some(&SubagentProviderTarget {
                provider: Some("groq-fast".to_string()),
                model: None,
            }),
            inherited,
        )
        .await
        .expect("resolved backend");

    assert_eq!(resolved.provider, "groq-fast");
    assert_eq!(resolved.model, "gpt-4o-mini");
}
