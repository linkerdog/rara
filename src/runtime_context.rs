mod tooling;

use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Context, Result, bail};
use rara_memory::vectordb::VectorDB;
use rara_tools::tool::ToolManager;

use self::tooling::{create_full_tool_manager, load_skill_manager, vector_db_uri_for_workspace};
use crate::agent::Agent;
use crate::config::{
    BuiltinPluginConfig, DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_MODEL, DEFAULT_GEMINI_BASE_URL,
    DEFAULT_GEMINI_MODEL, LocalEmbeddingPolicy, OpenAiEndpointKind, REASONING_SUMMARY_NONE,
    RaraConfig, ensure_rara_home_dir,
};
use crate::embedding::EmbeddingOverrideBackend;
use crate::google_oauth::GoogleOAuthManager;
use crate::hook_registry::HookRegistry;
use crate::hook_runtime::HookRuntime;
use crate::llm::{
    BedrockBackend, CodexBackend, EmbeddingBackend, GeminiBackend, LlmBackend, LlmEmbeddingBackend,
    MockLlm, OllamaBackend, OpenAiCompatibleBackend, fetch_model_context_window,
};
use crate::local_backend::{LocalLlmBackend, LocalProgressReporter};
use crate::local_model_server::{
    LocalModelServerEmbeddingBackend, LocalModelServerStatus, inspect_local_model_server_status,
    prepare_local_model_server_status_with_progress,
};
use crate::lsp_manager::LspManager;
use crate::mcp_connection_manager::McpConnectionManager;
use crate::mcp_tool_cache::McpToolCache;
use crate::prompt::{PromptRuntimeConfig, PromptSkillSummary};
use crate::protocol_sources::{PromptSourceRegistry, SkillSourceRegistry};
use crate::runtime_control::{ExtensionEvent, ExtensionReadinessSnapshot, RuntimeEvent};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::sandbox::SandboxManager;
use crate::session::SessionManager;
use crate::shell_env::capture_shell_environment_snapshot;
use crate::skill::SkillScope;
use crate::tools::agent::{
    AgentDefinitionCache, ResolvedSubagentBackend, SubagentBackendResolver, SubagentProviderTarget,
};
use crate::tui::state::GoalHandle;
use crate::workspace::WorkspaceMemory;

pub(crate) struct RuntimeBootstrap {
    pub backend: Arc<dyn LlmBackend>,
    pub embedding_backend: Arc<dyn EmbeddingBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub tool_manager: ToolManager,
    pub prompt_config: PromptRuntimeConfig,
    pub warnings: Vec<String>,
    pub sandbox_network_access: Arc<AtomicBool>,
    pub event_bus: Arc<RuntimeEventBus>,
    pub prompt_source_registry: Arc<PromptSourceRegistry>,
    pub skill_source_registry: Arc<SkillSourceRegistry>,
    pub hook_registry: Arc<HookRegistry>,
    pub hook_runtime: Arc<HookRuntime>,
    command_hook_registry: Arc<crate::hooks::HookRegistry>,
    pub goal_handle: GoalHandle,
    pub mcp_tool_cache: McpToolCache,
    pub mcp_manager: Arc<McpConnectionManager>,
    pub lsp_manager: Arc<LspManager>,
    pub agent_definitions: AgentDefinitionCache,
    extension_readiness: ExtensionReadinessSnapshot,
    plugin_dirs: Vec<PathBuf>,
    rara_home: Option<PathBuf>,
    builtin_plugins: BuiltinPluginConfig,
}

#[derive(Clone)]
pub(crate) struct ConfigSubagentBackendResolver {
    config: Arc<RaraConfig>,
}

impl ConfigSubagentBackendResolver {
    fn new(config: Arc<RaraConfig>) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl SubagentBackendResolver for ConfigSubagentBackendResolver {
    async fn resolve_backend(
        &self,
        target: Option<&SubagentProviderTarget>,
        inherited_backend: Arc<dyn LlmBackend>,
    ) -> std::result::Result<ResolvedSubagentBackend, rara_tools::tool::ToolError> {
        let Some(target) = target else {
            let model = inherited_backend
                .model_label()
                .or_else(|| self.config.model.clone())
                .unwrap_or_else(|| "inherit".to_string());
            return Ok(ResolvedSubagentBackend {
                backend: inherited_backend,
                provider: self.config.provider.clone(),
                model,
            });
        };

        let mut config = (*self.config).clone();
        let provider = target
            .provider
            .clone()
            .unwrap_or_else(|| config.provider.clone());
        if let Some(provider) = &target.provider {
            config.set_provider(provider.clone());
        }
        if let Some(model) = &target.model {
            config.set_model(Some(model.clone()));
        }
        let backend = build_backend_with_progress(&config, None)
            .await
            .map_err(|err| {
                rara_tools::tool::ToolError::ExecutionFailed(format!(
                    "failed to initialize sub-agent model backend: {err}"
                ))
            })?;
        let backend: Arc<dyn LlmBackend> = backend.into();
        let model = backend
            .model_label()
            .or_else(|| config.model.clone())
            .unwrap_or_else(|| "unknown".to_string());
        Ok(ResolvedSubagentBackend {
            backend,
            provider,
            model,
        })
    }
}

impl RuntimeBootstrap {
    pub(crate) async fn into_agent(self) -> Agent {
        let (agent, _, _, _, _, _, _, _, _, _, _) = self.into_parts_with_runtime_extensions().await;
        agent
    }

    #[allow(clippy::type_complexity)]
    // Bootstrap teardown returns the initialized runtime handles without
    // introducing another wrapper around RuntimeBootstrap itself.
    pub(crate) fn into_parts(
        self,
    ) -> (
        Agent,
        Vec<String>,
        Arc<AtomicBool>,
        GoalHandle,
        McpToolCache,
        Arc<McpConnectionManager>,
        Arc<PromptSourceRegistry>,
        Arc<SkillSourceRegistry>,
        Arc<HookRegistry>,
        Arc<HookRuntime>,
        Arc<LspManager>,
    ) {
        let hook_workspace_root = self.workspace.root.clone();
        let mut agent = Agent::new_with_embedding_backend_and_agent_definitions(
            self.tool_manager,
            self.backend,
            self.embedding_backend,
            self.vdb,
            self.session_manager,
            self.workspace,
            self.agent_definitions,
        );
        agent.set_prompt_config(self.prompt_config);
        agent.set_prompt_source_registry(self.prompt_source_registry.clone());
        agent.set_skill_source_registry(self.skill_source_registry.clone());
        agent.set_lsp_manager(self.lsp_manager.clone());
        agent.set_hook_context(
            self.command_hook_registry,
            crate::hooks::HookSandbox {
                workspace_root: hook_workspace_root,
                ..crate::hooks::HookSandbox::default()
            },
            self.hook_runtime.clone(),
        );
        (
            agent,
            self.warnings,
            self.sandbox_network_access,
            self.goal_handle,
            self.mcp_tool_cache,
            self.mcp_manager,
            self.prompt_source_registry,
            self.skill_source_registry,
            self.hook_registry,
            self.hook_runtime,
            self.lsp_manager,
        )
    }

    #[allow(clippy::type_complexity)]
    pub(crate) async fn into_parts_with_runtime_extensions(
        self,
    ) -> (
        Agent,
        Vec<String>,
        Arc<AtomicBool>,
        GoalHandle,
        McpToolCache,
        Arc<McpConnectionManager>,
        Arc<PromptSourceRegistry>,
        Arc<SkillSourceRegistry>,
        Arc<HookRegistry>,
        Arc<HookRuntime>,
        Arc<LspManager>,
    ) {
        let workspace_root = self.workspace.root.clone();
        let plugin_dirs = self.plugin_dirs.clone();
        self.into_parts_with_runtime_extensions_for_plugin_dirs(&workspace_root, &plugin_dirs)
            .await
    }

    pub(crate) async fn into_runtime_client_parts(
        mut self,
    ) -> (
        (
            Agent,
            Vec<String>,
            Arc<AtomicBool>,
            GoalHandle,
            McpToolCache,
            Arc<McpConnectionManager>,
            Arc<PromptSourceRegistry>,
            Arc<SkillSourceRegistry>,
            Arc<HookRegistry>,
            Arc<HookRuntime>,
            Arc<LspManager>,
        ),
        Vec<PathBuf>,
    ) {
        let plugin_dirs = std::mem::take(&mut self.plugin_dirs);
        let workspace_root = self.workspace.root.clone();
        let parts = self
            .into_parts_with_runtime_extensions_for_plugin_dirs(&workspace_root, &plugin_dirs)
            .await;
        (parts, plugin_dirs)
    }

    async fn into_parts_with_runtime_extensions_for_plugin_dirs(
        self,
        workspace_root: &Path,
        plugin_dirs: &[PathBuf],
    ) -> (
        Agent,
        Vec<String>,
        Arc<AtomicBool>,
        GoalHandle,
        McpToolCache,
        Arc<McpConnectionManager>,
        Arc<PromptSourceRegistry>,
        Arc<SkillSourceRegistry>,
        Arc<HookRegistry>,
        Arc<HookRuntime>,
        Arc<LspManager>,
    ) {
        let rara_home = self.rara_home.clone();
        let builtin_plugins = self.builtin_plugins.clone();
        let hook_runtime = self.hook_runtime.clone();
        let event_bus = self.event_bus.clone();
        let mut extension_readiness = self.extension_readiness.clone();
        let mut parts = self.into_parts();
        let plugin_hook_runtime = crate::plugin_middleware::register_plugin_hooks(
            &hook_runtime,
            rara_home,
            workspace_root,
            plugin_dirs,
            &builtin_plugins,
            &parts.0.session_id,
        )
        .await;
        extension_readiness.hook_count = plugin_hook_runtime.hook_count();
        extension_readiness.command_count = plugin_hook_runtime.command_summaries().len();
        parts.0.set_plugin_hook_runtime(plugin_hook_runtime);
        event_bus.publish_control(RuntimeEvent::Extension(ExtensionEvent::ReadinessUpdated {
            snapshot: extension_readiness,
        }));
        parts
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RuntimeBootstrapOptions {
    pub plugin_dirs: Vec<PathBuf>,
}

impl RuntimeBootstrapOptions {
    pub(crate) fn with_plugin_dirs(plugin_dirs: Vec<PathBuf>) -> Self {
        Self { plugin_dirs }
    }
}

pub(crate) async fn initialize_rara_context(
    config: &RaraConfig,
    progress: Option<LocalProgressReporter>,
) -> Result<RuntimeBootstrap> {
    initialize_rara_context_for_workspace(config, None, progress).await
}

/// Build a runtime for an explicit workspace without mutating the process cwd.
pub(crate) async fn initialize_rara_context_for_workspace(
    config: &RaraConfig,
    workspace_root: Option<&Path>,
    progress: Option<LocalProgressReporter>,
) -> Result<RuntimeBootstrap> {
    initialize_rara_context_with_local_embedding_bootstrap(
        config,
        workspace_root,
        progress,
        LocalEmbeddingBootstrap::Prepare,
    )
    .await
}

pub(crate) async fn initialize_rara_context_for_workspace_with_options(
    config: &RaraConfig,
    workspace_root: Option<&Path>,
    progress: Option<LocalProgressReporter>,
    options: RuntimeBootstrapOptions,
) -> Result<RuntimeBootstrap> {
    initialize_rara_context_with_options_and_local_embedding_bootstrap(
        config,
        workspace_root,
        progress,
        LocalEmbeddingBootstrap::Prepare,
        options,
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalEmbeddingBootstrap {
    Prepare,
    InspectOnly,
}

pub(crate) async fn initialize_rara_context_with_local_embedding_bootstrap(
    config: &RaraConfig,
    workspace_root: Option<&Path>,
    progress: Option<LocalProgressReporter>,
    local_embedding_bootstrap: LocalEmbeddingBootstrap,
) -> Result<RuntimeBootstrap> {
    initialize_rara_context_with_options_and_local_embedding_bootstrap(
        config,
        workspace_root,
        progress,
        local_embedding_bootstrap,
        RuntimeBootstrapOptions::default(),
    )
    .await
}

pub(crate) async fn initialize_rara_context_with_options_and_local_embedding_bootstrap(
    config: &RaraConfig,
    workspace_root: Option<&Path>,
    progress: Option<LocalProgressReporter>,
    local_embedding_bootstrap: LocalEmbeddingBootstrap,
    options: RuntimeBootstrapOptions,
) -> Result<RuntimeBootstrap> {
    let embedding_progress = progress.clone();
    let chat_backend = build_backend_with_progress(config, progress).await?;
    let chat_backend: Arc<dyn LlmBackend> = chat_backend.into();
    let mut embedding_warnings = Vec::new();
    let (backend, embedding_backend) = build_embedding_backends(
        config,
        chat_backend,
        local_embedding_bootstrap,
        embedding_progress,
        &mut embedding_warnings,
    )
    .await?;

    let workspace = match workspace_root {
        Some(root) => Arc::new(WorkspaceMemory::from_paths(
            root.to_path_buf(),
            rara_config::workspace_data_dir_for(root)?,
        )),
        None => Arc::new(WorkspaceMemory::new()?),
    };
    let vdb = Arc::new(VectorDB::new(&vector_db_uri_for_workspace(&workspace)));
    let session_manager = Arc::new(SessionManager::new_for_rara_dir(
        workspace.rara_dir.clone(),
    )?);
    let shell_env = capture_shell_environment_snapshot().await;
    let sandbox_manager = Arc::new(SandboxManager::new_with_command_path(
        shell_env.env.get("PATH").cloned(),
    )?);

    let config_manager = crate::config::ConfigManager::new()?;
    let rara_home = ensure_rara_home_dir()?;
    let plugins = crate::plugin_middleware::discover_runtime_plugins(
        Some(&rara_home),
        &workspace.root,
        &options.plugin_dirs,
        &config.builtin_plugins,
    );
    let plugin_skill_roots = crate::plugin_middleware::plugin_skill_roots(&plugins);
    let plugin_agent_records = crate::plugin_middleware::plugin_agent_records(&plugins);

    let mut prompt_config = PromptRuntimeConfig::from_config(config);
    let skill_manager = load_skill_manager(&mut prompt_config.warnings, &plugin_skill_roots);
    let skill_summaries = skill_manager
        .read()
        .map_err(|err| anyhow::anyhow!("skill manager lock failed: {err}"))?
        .list_summaries();
    let extension_skill_count = skill_summaries
        .iter()
        .filter(|skill| skill.scope == rara_skills::SkillScope::Plugin)
        .count();
    prompt_config.available_skills = skill_summaries
        .into_iter()
        .map(|skill| {
            let scope = match skill.scope {
                rara_skills::SkillScope::Global | rara_skills::SkillScope::Home => "global",
                rara_skills::SkillScope::Workspace
                | rara_skills::SkillScope::Repo
                | rara_skills::SkillScope::Cwd => "workspace",
                rara_skills::SkillScope::Plugin => "plugin",
                rara_skills::SkillScope::System => "system",
            };
            PromptSkillSummary {
                name: skill.name.clone(),
                title: Some(skill.name),
                description: skill.description,
                scope: scope.to_string(),
                disable_model_invocation: false,
            }
        })
        .collect();

    let sandbox_network_access = Arc::new(AtomicBool::new(
        config.sandbox_workspace_write.network_access,
    ));

    let event_bus = Arc::new(RuntimeEventBus::new(256));
    let hook_runtime = Arc::new(HookRuntime::new(event_bus.clone()));
    hook_runtime.start();
    let prompt_source_registry = Arc::new(PromptSourceRegistry::new(event_bus.clone()));
    let skill_source_registry = Arc::new(SkillSourceRegistry::new(event_bus.clone()));
    let hook_registry = Arc::new(HookRegistry::new(event_bus.clone()));
    let goal_handle: GoalHandle = Arc::new(std::sync::RwLock::new(None));
    let mcp_tool_cache = McpToolCache::new();
    mcp_tool_cache.clear();
    let lsp_manager = Arc::new(LspManager::new(workspace.root.clone()));

    let mut mcp_registry = config_manager
        .load_mcp_registry_for_project(&workspace.root)
        .unwrap_or_else(|_| crate::config::McpRegistry::empty());
    crate::plugin_middleware::append_plugin_mcp_configs(
        &mut mcp_registry,
        Some(&rara_home),
        &workspace.root,
        &options.plugin_dirs,
        &config.builtin_plugins,
    )?;
    let extension_mcp_server_count = mcp_registry
        .servers
        .values()
        .filter(|server| {
            matches!(
                server.source.scope,
                crate::config::McpServerScope::Plugin | crate::config::McpServerScope::Builtin
            )
        })
        .count();
    let mcp_registry = Arc::new(mcp_registry);

    let mcp_manager = Arc::new(McpConnectionManager::new(
        mcp_registry.clone(),
        event_bus.clone(),
    ));

    // Discover file-based hooks and inject into prompt config
    let mut file_hooks = crate::hooks::HookRegistry::new();
    file_hooks.discover_repo_hooks(&workspace.root);
    prompt_config.hook_prompt_entries = file_hooks
        .hooks
        .values()
        .filter(|hook| hook.source_path != ".claude/settings.json")
        .map(|h| rara_instructions::HookPromptEntry {
            phase: h.phase,
            body: format!("## {}\n\n{}", h.phase.as_str(), h.body),
        })
        .collect();
    let file_hook_warnings = file_hooks.load_warnings.clone();
    let command_hook_registry = Arc::new(file_hooks);
    let extension_agent_count = crate::agents_ext::AgentRegistry::from_records(
        plugin_agent_records.clone(),
        &workspace.root,
    )
    .agents
    .len();
    let agent_definitions =
        AgentDefinitionCache::load_with_records(workspace.root.clone(), plugin_agent_records);
    let subagent_backend_resolver: Arc<dyn SubagentBackendResolver> =
        Arc::new(ConfigSubagentBackendResolver::new(Arc::new(config.clone())));
    let tool_manager = create_full_tool_manager(
        backend.clone(),
        embedding_backend.clone(),
        vdb.clone(),
        session_manager.clone(),
        workspace.clone(),
        sandbox_manager.clone(),
        skill_manager,
        plugin_skill_roots,
        prompt_config.clone(),
        Arc::new(shell_env.env),
        sandbox_network_access.clone(),
        goal_handle.clone(),
        mcp_tool_cache.clone(),
        hook_runtime.clone(),
        lsp_manager.clone(),
        subagent_backend_resolver.clone(),
        agent_definitions.clone(),
    );
    let mut warnings = prompt_config.warnings.clone();
    warnings.extend(embedding_warnings);
    warnings.extend(file_hook_warnings);

    Ok(RuntimeBootstrap {
        backend,
        embedding_backend,
        vdb,
        session_manager,
        workspace,
        tool_manager,
        prompt_config,
        warnings,
        sandbox_network_access,
        event_bus,
        prompt_source_registry,
        skill_source_registry,
        hook_registry,
        hook_runtime,
        command_hook_registry,
        goal_handle,
        mcp_tool_cache,
        mcp_manager,
        lsp_manager,
        agent_definitions,
        extension_readiness: ExtensionReadinessSnapshot {
            plugin_count: plugins.len(),
            hook_count: 0,
            skill_count: extension_skill_count,
            command_count: 0,
            agent_count: extension_agent_count,
            mcp_server_count: extension_mcp_server_count,
        },
        plugin_dirs: options.plugin_dirs,
        rara_home: Some(rara_home),
        builtin_plugins: config.builtin_plugins.clone(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EmbeddingRoute {
    CurrentLlmBackend,
    LocalModelServer,
}

fn embedding_route_for_config(config: &RaraConfig) -> EmbeddingRoute {
    match config.local_embeddings {
        LocalEmbeddingPolicy::Off | LocalEmbeddingPolicy::Provider => {
            return EmbeddingRoute::CurrentLlmBackend;
        }
        LocalEmbeddingPolicy::Local => {
            return EmbeddingRoute::LocalModelServer;
        }
        LocalEmbeddingPolicy::Auto => {}
    }
    match config.provider.as_str() {
        "codex" | "mock" => EmbeddingRoute::CurrentLlmBackend,
        provider if RaraConfig::is_openai_compatible_family(provider) => {
            let endpoint_kind = config
                .active_openai_profile_kind()
                .unwrap_or(match provider {
                    "deepseek" => OpenAiEndpointKind::Deepseek,
                    "kimi" => OpenAiEndpointKind::Kimi,
                    "openrouter" => OpenAiEndpointKind::Openrouter,
                    _ => OpenAiEndpointKind::Custom,
                });
            match endpoint_kind {
                OpenAiEndpointKind::Deepseek | OpenAiEndpointKind::Kimi => {
                    EmbeddingRoute::LocalModelServer
                }
                OpenAiEndpointKind::Custom | OpenAiEndpointKind::Openrouter => {
                    EmbeddingRoute::CurrentLlmBackend
                }
            }
        }
        provider if is_configured_openai_compatible_provider(config, provider) => {
            EmbeddingRoute::CurrentLlmBackend
        }
        _ => EmbeddingRoute::LocalModelServer,
    }
}

fn is_configured_openai_compatible_provider(config: &RaraConfig, provider: &str) -> bool {
    if RaraConfig::is_openai_compatible_family(provider) {
        return false;
    }

    config
        .base_url
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn config_requires_local_embedding_sidecar(config: &RaraConfig) -> bool {
    matches!(
        embedding_route_for_config(config),
        EmbeddingRoute::LocalModelServer
    )
}

async fn build_embedding_backends(
    config: &RaraConfig,
    chat_backend: Arc<dyn LlmBackend>,
    bootstrap: LocalEmbeddingBootstrap,
    progress: Option<LocalProgressReporter>,
    warnings: &mut Vec<String>,
) -> Result<(Arc<dyn LlmBackend>, Arc<dyn EmbeddingBackend>)> {
    match embedding_route_for_config(config) {
        EmbeddingRoute::CurrentLlmBackend => {
            let embedding_backend: Arc<dyn EmbeddingBackend> =
                Arc::new(LlmEmbeddingBackend::new(chat_backend.clone()));
            Ok((chat_backend, embedding_backend))
        }
        EmbeddingRoute::LocalModelServer => {
            let rara_home = ensure_rara_home_dir()?;
            // `local_model_server_status_for_bootstrap` uses a `reqwest::blocking` client, which
            // spins up and drops its own Tokio runtime. Dropping a runtime inside the async context
            // would panic, so run the probe on a blocking thread where that is allowed.
            let status = {
                let rara_home = rara_home.clone();
                tokio::task::spawn_blocking(move || {
                    local_model_server_status_for_bootstrap(&rara_home, bootstrap, &progress)
                })
                .await
                .context("join local model server bootstrap status")?
            };
            if status.state == crate::local_model_server::LocalModelServerState::Error {
                warnings.push(format!(
                    "local embedding backend bootstrap reported: {}",
                    status.detail
                ));
            } else {
                warnings.push(format!(
                    "embedding · {} · {} · {:?}",
                    status.backend, status.model, status.state
                ));
            }
            let embedding_backend: Arc<dyn EmbeddingBackend> = Arc::new(
                LocalModelServerEmbeddingBackend::from_initial_status(rara_home, status)?,
            );
            let backend: Arc<dyn LlmBackend> = Arc::new(EmbeddingOverrideBackend::new(
                chat_backend,
                embedding_backend.clone(),
            ));
            Ok((backend, embedding_backend))
        }
    }
}

fn local_model_server_status_for_bootstrap(
    rara_home: &std::path::Path,
    bootstrap: LocalEmbeddingBootstrap,
    progress: &Option<LocalProgressReporter>,
) -> LocalModelServerStatus {
    report_local_embedding_progress(progress, "Embedding · checking local model server");
    let status = match bootstrap {
        LocalEmbeddingBootstrap::Prepare => {
            report_local_embedding_progress(progress, "Embedding · preparing local model server");
            prepare_local_model_server_status_with_progress(rara_home, progress.clone())
        }
        LocalEmbeddingBootstrap::InspectOnly => inspect_local_model_server_status(rara_home),
    };
    report_local_embedding_progress(
        progress,
        format!("Embedding · {:?} · {}", status.state, status.detail),
    );
    status
}

fn report_local_embedding_progress(
    progress: &Option<LocalProgressReporter>,
    message: impl Into<String>,
) {
    if let Some(callback) = progress {
        callback(message.into());
    }
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
                .unwrap_or(match provider {
                    "deepseek" => OpenAiEndpointKind::Deepseek,
                    "kimi" => OpenAiEndpointKind::Kimi,
                    "openrouter" => OpenAiEndpointKind::Openrouter,
                    _ => OpenAiEndpointKind::Custom,
                });
            let model = config
                .model
                .clone()
                .unwrap_or_else(|| kind.default_model().to_string());
            let surface = config.effective_provider_surface();
            let base_url = surface
                .base_url
                .value
                .unwrap_or_else(|| kind.default_base_url())
                .to_string();
            build_openai_compatible_backend(config, base_url, model, kind).await
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
            build_openai_compatible_backend(config, base_url, model, OpenAiEndpointKind::Custom)
                .await
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
        provider if is_configured_openai_compatible_provider(config, provider) => {
            let model = config
                .model
                .clone()
                .with_context(|| format!("Model required for configured provider '{provider}'"))?;
            let base_url = config.base_url.clone().with_context(|| {
                format!("Base URL required for configured provider '{provider}'")
            })?;
            build_openai_compatible_backend(config, base_url, model, OpenAiEndpointKind::Custom)
                .await
        }
        other => bail!("Unsupported provider '{other}'"),
    }
}

async fn build_openai_compatible_backend(
    config: &RaraConfig,
    base_url: String,
    model: String,
    kind: OpenAiEndpointKind,
) -> Result<Box<dyn LlmBackend>> {
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

fn ollama_thinking_enabled(config: &RaraConfig) -> bool {
    match config.reasoning_summary.as_deref() {
        Some(REASONING_SUMMARY_NONE) => false,
        _ => config.thinking.unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use tempfile::tempdir;

    use super::{
        ConfigSubagentBackendResolver, EmbeddingRoute, RuntimeBootstrapOptions,
        build_backend_with_progress, config_requires_local_embedding_sidecar,
        embedding_route_for_config, initialize_rara_context, ollama_thinking_enabled,
        vector_db_uri_for_workspace,
    };
    use crate::config::{
        DEFAULT_REASONING_SUMMARY, LocalEmbeddingPolicy, ProviderConfigState,
        REASONING_SUMMARY_NONE, RaraConfig,
    };
    use crate::llm::{LlmBackend, MockLlm};
    use crate::tools::agent::{SubagentBackendResolver, SubagentProviderTarget};
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

    #[test]
    fn embedding_route_keeps_local_sidecar_off_by_default() {
        let deepseek = RaraConfig {
            provider: "deepseek".to_string(),
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&deepseek),
            EmbeddingRoute::CurrentLlmBackend
        );
        assert!(!config_requires_local_embedding_sidecar(&deepseek));
    }

    #[test]
    fn embedding_route_disables_local_sidecar_when_policy_is_off() {
        let deepseek = RaraConfig {
            provider: "deepseek".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Off,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&deepseek),
            EmbeddingRoute::CurrentLlmBackend
        );
        assert!(!config_requires_local_embedding_sidecar(&deepseek));
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

    #[test]
    fn embedding_route_prefers_local_sidecar_when_auto_policy_is_enabled() {
        let deepseek = RaraConfig {
            provider: "deepseek".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Auto,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&deepseek),
            EmbeddingRoute::LocalModelServer
        );

        let local = RaraConfig {
            provider: "local".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Auto,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&local),
            EmbeddingRoute::LocalModelServer
        );

        let gemini_code_assist = RaraConfig {
            provider: "gemini-code-assist".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Auto,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&gemini_code_assist),
            EmbeddingRoute::LocalModelServer
        );
    }

    #[test]
    fn embedding_route_honors_explicit_provider_and_local_overrides() {
        let deepseek_provider = RaraConfig {
            provider: "deepseek".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Provider,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&deepseek_provider),
            EmbeddingRoute::CurrentLlmBackend
        );
        assert!(!config_requires_local_embedding_sidecar(&deepseek_provider));

        let codex_local = RaraConfig {
            provider: "codex".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Local,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&codex_local),
            EmbeddingRoute::LocalModelServer
        );
        assert!(config_requires_local_embedding_sidecar(&codex_local));
    }

    #[test]
    fn embedding_route_reuses_provider_embedding_for_supported_openai_like_surfaces() {
        let codex = RaraConfig {
            provider: "codex".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Auto,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&codex),
            EmbeddingRoute::CurrentLlmBackend
        );

        let openai_compatible = RaraConfig {
            provider: "openai-compatible".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Auto,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&openai_compatible),
            EmbeddingRoute::CurrentLlmBackend
        );

        let mock = RaraConfig {
            provider: "mock".to_string(),
            local_embeddings: LocalEmbeddingPolicy::Auto,
            ..Default::default()
        };
        assert_eq!(
            embedding_route_for_config(&mock),
            EmbeddingRoute::CurrentLlmBackend
        );
    }
}
