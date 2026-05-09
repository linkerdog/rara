mod tooling;

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Context, Result, bail};
use rara_memory::vectordb::VectorDB;
use rara_tools::tool::ToolManager;

use self::tooling::{create_full_tool_manager, load_skill_manager, vector_db_uri_for_workspace};
use crate::agent::Agent;
use crate::config::{
    DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_MODEL, DEFAULT_GEMINI_BASE_URL, DEFAULT_GEMINI_MODEL,
    OpenAiEndpointKind, REASONING_SUMMARY_NONE, RaraConfig, ensure_rara_home_dir,
};
use crate::google_oauth::GoogleOAuthManager;
use crate::llm::{
    BedrockBackend, CodexBackend, GeminiBackend, LlmBackend, MockLlm, OllamaBackend,
    OpenAiCompatibleBackend, fetch_model_context_window,
};
use crate::local_backend::{LocalLlmBackend, LocalProgressReporter};
use crate::mcp_tool_cache::McpToolCache;
use crate::prompt::{PromptRuntimeConfig, PromptSkillSummary};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::sandbox::SandboxManager;
use crate::session::SessionManager;
use crate::shell_env::capture_shell_environment_snapshot;
use crate::skill::SkillScope;
use crate::tui::state::GoalHandle;
use crate::workspace::WorkspaceMemory;

pub(crate) struct RuntimeBootstrap {
    pub backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub tool_manager: ToolManager,
    pub prompt_config: PromptRuntimeConfig,
    pub warnings: Vec<String>,
    pub sandbox_network_access: Arc<AtomicBool>,
    pub event_bus: Arc<RuntimeEventBus>,
    pub goal_handle: GoalHandle,
    pub mcp_tool_cache: McpToolCache,
}

impl RuntimeBootstrap {
    pub(crate) fn into_agent(self) -> Agent {
        let (agent, _, _, _, _) = self.into_parts();
        agent
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Agent,
        Vec<String>,
        Arc<AtomicBool>,
        GoalHandle,
        McpToolCache,
    ) {
        let mut agent = Agent::new(
            self.tool_manager,
            self.backend,
            self.vdb,
            self.session_manager,
            self.workspace,
        );
        agent.set_prompt_config(self.prompt_config);
        (
            agent,
            self.warnings,
            self.sandbox_network_access,
            self.goal_handle,
            self.mcp_tool_cache,
        )
    }
}

pub(crate) async fn initialize_rara_context(
    config: &RaraConfig,
    progress: Option<LocalProgressReporter>,
) -> Result<RuntimeBootstrap> {
    let backend = build_backend_with_progress(config, progress).await?;
    let backend: Arc<dyn LlmBackend> = backend.into();

    let workspace = Arc::new(WorkspaceMemory::new()?);
    let vdb = Arc::new(VectorDB::new(&vector_db_uri_for_workspace(&workspace)));
    let session_manager = Arc::new(SessionManager::new()?);
    let shell_env = capture_shell_environment_snapshot().await;
    let sandbox_manager = Arc::new(SandboxManager::new_with_command_path(
        shell_env.env.get("PATH").cloned(),
    )?);

    let mut prompt_config = PromptRuntimeConfig::from_config(config);
    let skill_manager = load_skill_manager(&mut prompt_config.warnings);
    prompt_config.available_skills = skill_manager
        .list_summaries()
        .into_iter()
        .map(|skill| {
            let scope = match skill.scope {
                SkillScope::Home => "home",
                SkillScope::Repo => "repo",
                SkillScope::Cwd => "cwd",
                SkillScope::System => "system",
            };
            PromptSkillSummary {
                name: skill.name,
                title: skill.title,
                description: skill.description,
                display_path: skill.display_path,
                scope: scope.to_string(),
                disable_model_invocation: skill.disable_model_invocation,
            }
        })
        .collect();

    let sandbox_network_access = Arc::new(AtomicBool::new(
        config.sandbox_workspace_write.network_access,
    ));

    let event_bus = Arc::new(RuntimeEventBus::new(256));
    let goal_handle: GoalHandle = Arc::new(std::sync::RwLock::new(None));
    let mcp_tool_cache = McpToolCache::new();
    mcp_tool_cache.clear();

    let tool_manager = create_full_tool_manager(
        backend.clone(),
        vdb.clone(),
        session_manager.clone(),
        workspace.clone(),
        sandbox_manager.clone(),
        skill_manager,
        prompt_config.clone(),
        Arc::new(shell_env.env),
        sandbox_network_access.clone(),
        goal_handle.clone(),
        mcp_tool_cache.clone(),
    );
    let warnings = prompt_config.warnings.clone();

    Ok(RuntimeBootstrap {
        backend,
        vdb,
        session_manager,
        workspace,
        tool_manager,
        prompt_config,
        warnings,
        sandbox_network_access,
        event_bus,
        goal_handle,
        mcp_tool_cache,
    })
}

pub(crate) async fn build_backend(config: &RaraConfig) -> Result<Box<dyn LlmBackend>> {
    build_backend_with_progress(config, None).await
}

pub(crate) async fn build_backend_with_progress(
    config: &RaraConfig,
    progress: Option<LocalProgressReporter>,
) -> Result<Box<dyn LlmBackend>> {
    match config.provider.as_str() {
        "codex" => Ok(Box::new(
            CodexBackend::new(
                config.api_key_secret(),
                config
                    .base_url
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CODEX_BASE_URL.to_string()),
                config
                    .model
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string()),
                config.reasoning_effort.clone(),
            )?
            .with_auxiliary_model(config.auxiliary_model.clone()),
        )),
        provider if RaraConfig::is_openai_compatible_family(provider) => {
            let kind = config
                .active_openai_profile_kind()
                .unwrap_or_else(|| match provider {
                    "deepseek" => OpenAiEndpointKind::Deepseek,
                    "kimi" => OpenAiEndpointKind::Kimi,
                    "openrouter" => OpenAiEndpointKind::Openrouter,
                    _ => OpenAiEndpointKind::Custom,
                });
            let model = config
                .model
                .clone()
                .unwrap_or_else(|| kind.default_model().to_string());
            let base_url = config
                .base_url
                .clone()
                .unwrap_or_else(|| kind.default_base_url().to_string());
            let mut backend = OpenAiCompatibleBackend::new_with_endpoint_kind_and_reasoning(
                config.api_key_secret(),
                base_url.clone(),
                model.clone(),
                kind,
                config.reasoning_effort.clone(),
                config.thinking,
            )?
            .with_auxiliary_model(config.auxiliary_model.clone());
            if backend.context_budget(&[], &[]).is_none() {
                backend.context_window_override = fetch_model_context_window(
                    &backend.client,
                    &base_url,
                    backend.api_key.as_ref(),
                    &model,
                )
                .await;
            }
            Ok(Box::new(backend))
        }
        "ollama" | "ollama-native" => Ok(Box::new(OllamaBackend::new(
            config
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".to_string()),
            config
                .model
                .clone()
                .context("Model required for Ollama provider")?,
            ollama_thinking_enabled(config),
            config.num_ctx,
        )?)),
        "ollama-openai" => Ok(Box::new(
            OpenAiCompatibleBackend::new(
                config.api_key_secret(),
                config
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434".to_string()),
                config
                    .model
                    .clone()
                    .context("Model required for Ollama OpenAI provider")?,
            )?
            .with_auxiliary_model(config.auxiliary_model.clone()),
        )),
        "gemini" => {
            let model = config
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_GEMINI_MODEL.to_string());
            let base_url = config
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_GEMINI_BASE_URL.to_string());
            let mut backend = OpenAiCompatibleBackend::new_with_endpoint_kind_and_reasoning(
                config.api_key_secret(),
                base_url.clone(),
                model.clone(),
                OpenAiEndpointKind::Custom,
                config.reasoning_effort.clone(),
                config.thinking,
            )?
            .with_auxiliary_model(config.auxiliary_model.clone());
            if backend.context_budget(&[], &[]).is_none() {
                backend.context_window_override = fetch_model_context_window(
                    &backend.client,
                    &base_url,
                    backend.api_key.as_ref(),
                    &model,
                )
                .await;
            }
            Ok(Box::new(backend))
        }
        "gemini-code-assist" => {
            let model = config
                .model
                .clone()
                .unwrap_or_else(|| "gemini-2.5-flash".to_string());
            let home = ensure_rara_home_dir()?;
            let oauth = GoogleOAuthManager::new(home)?;
            let backend = GeminiBackend::with_oauth(oauth, model)?;
            Ok(Box::new(backend))
        }
        "gemma4" | "qwen3" | "qwn3" | "local" | "local-candle" => {
            let config = config.clone();
            let progress = progress.clone();
            let backend = tokio::task::spawn_blocking(move || {
                LocalLlmBackend::from_config_with_progress(&config, progress)
            })
            .await??;
            Ok(Box::new(backend))
        }
        "bedrock" => Ok(Box::new(
            BedrockBackend::new(
                config.aws_region.clone(),
                config
                    .model
                    .clone()
                    .context("Model required for Bedrock provider")?,
            )
            .await?,
        )),
        "mock" => Ok(Box::new(MockLlm)),
        other => bail!("Unsupported provider '{other}'"),
    }
}

fn ollama_thinking_enabled(config: &RaraConfig) -> bool {
    match config.reasoning_summary.as_deref() {
        Some(REASONING_SUMMARY_NONE) => false,
        _ => config.thinking.unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        build_backend_with_progress, initialize_rara_context, ollama_thinking_enabled,
        vector_db_uri_for_workspace,
    };
    use crate::config::{DEFAULT_REASONING_SUMMARY, REASONING_SUMMARY_NONE, RaraConfig};
    use crate::workspace::WorkspaceMemory;

    #[test]
    fn vector_db_uri_is_workspace_scoped() {
        let temp = tempdir().expect("tempdir");
        let workspace =
            WorkspaceMemory::from_paths(temp.path().join("repo"), temp.path().join(".rara"));

        assert_eq!(
            vector_db_uri_for_workspace(&workspace),
            temp.path()
                .join(".rara")
                .join("lancedb")
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
}
