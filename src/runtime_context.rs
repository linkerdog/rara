mod tooling;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, atomic::AtomicBool};

use anyhow::{Context, Result, bail};
use rara_memory::memory_handle::MemoryHandle;
use rara_skills::SkillManager;
use rara_tools::tool::ToolManager;

use self::tooling::{create_full_tool_manager, load_skill_manager};
use crate::agent::Agent;
use crate::config::{
    BuiltinPluginConfig, DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_MODEL, DEFAULT_GEMINI_BASE_URL,
    DEFAULT_GEMINI_MODEL, MultiAgentPolicy, OpenAiEndpointKind, REASONING_SUMMARY_NONE, RaraConfig,
    ensure_rara_home_dir,
};
use crate::google_oauth::GoogleOAuthManager;
use crate::hook_registry::HookRegistry;
use crate::hook_runtime::HookRuntime;
use crate::llm::{
    BedrockBackend, CodexBackend, GeminiBackend, LlmBackend, Message, MockLlm, OllamaBackend,
    OpenAiCompatibleBackend, fetch_model_context_window,
};
use crate::local_backend::{LocalLlmBackend, LocalProgressReporter};
use crate::lsp_manager::LspManager;
use crate::mcp_connection_manager::McpConnectionManager;
use crate::mcp_tool_cache::McpToolCache;
use crate::prompt::{PromptRuntimeConfig, PromptSkillSummary};
use crate::protocol_sources::{PromptSourceRegistry, SkillSourceRegistry};
use crate::runtime_control::{ExtensionEvent, ExtensionReadinessSnapshot, RuntimeEvent};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::runtime_session::RuntimeSessionProfile;
use crate::sandbox::SandboxManager;
use crate::session::SessionManager;
use crate::shell_env::capture_shell_environment_snapshot;
use crate::skill::SkillScope;
use crate::tools::agent::{
    AgentDefinitionCache, AgentTreeConfig, AgentTreeControl, ResolvedSubagentBackend,
    SubagentBackendResolver, SubagentProviderTarget,
};
use crate::tui::state::GoalHandle;
use crate::workspace::WorkspaceMemory;

pub(crate) struct RuntimeBootstrap {
    pub backend: Arc<dyn LlmBackend>,
    pub memory_handle: Arc<MemoryHandle>,
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
    pub agent_tree_control: Arc<AgentTreeControl>,
    extension_readiness: ExtensionReadinessSnapshot,
    plugin_dirs: Vec<PathBuf>,
    rara_home: Option<PathBuf>,
    builtin_plugins: BuiltinPluginConfig,
    extension_discovery: bool,
    session_id: Option<String>,
    initial_transcript: Vec<Message>,
    transcript_persistence: bool,
    memory_facilities: bool,
}

/// Named ownership bundle produced by runtime assembly for one session.
///
/// Keeping this as a struct prevents presentation and protocol adapters from
/// depending on tuple position when the session graph evolves.
pub(crate) struct RuntimeSessionComponents {
    pub(crate) agent: Agent,
    pub(crate) warnings: Vec<String>,
    pub(crate) sandbox_network_access: Arc<AtomicBool>,
    pub(crate) goal_handle: GoalHandle,
    pub(crate) mcp_tool_cache: McpToolCache,
    pub(crate) mcp_manager: Arc<McpConnectionManager>,
    pub(crate) prompt_source_registry: Arc<PromptSourceRegistry>,
    pub(crate) skill_source_registry: Arc<SkillSourceRegistry>,
    pub(crate) hook_registry: Arc<HookRegistry>,
    pub(crate) hook_runtime: Arc<HookRuntime>,
    pub(crate) lsp_manager: Arc<LspManager>,
    pub(crate) event_bus: Arc<RuntimeEventBus>,
    pub(crate) memory_config: rara_config::NowledgeMemPluginConfig,
    pub(crate) explicit_plugin_dirs: Vec<PathBuf>,
}

#[derive(Clone)]
pub(crate) struct ConfigSubagentBackendResolver {
    config: Arc<RaraConfig>,
    rara_home: Option<PathBuf>,
}

impl ConfigSubagentBackendResolver {
    #[cfg(test)]
    fn new(config: Arc<RaraConfig>) -> Self {
        Self {
            config,
            rara_home: None,
        }
    }

    fn new_for_rara_home(config: Arc<RaraConfig>, rara_home: PathBuf) -> Self {
        Self {
            config,
            rara_home: Some(rara_home),
        }
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
        let backend =
            build_backend_with_progress_for_home(&config, None, self.rara_home.as_deref())
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
    #[cfg(test)]
    pub(crate) async fn into_agent(self) -> Agent {
        self.into_session_components().await.agent
    }

    fn into_session_components_without_extensions(
        self,
        explicit_plugin_dirs: Vec<PathBuf>,
    ) -> RuntimeSessionComponents {
        let hook_workspace_root = self.workspace.root.clone();
        let memory_config = self.builtin_plugins.nowledge_mem.clone();
        let mut agent = Agent::new_with_agent_definitions(
            self.tool_manager,
            self.backend,
            self.memory_handle,
            self.session_manager,
            self.workspace,
            self.agent_definitions,
        );
        agent.set_prompt_config(self.prompt_config);
        agent.set_prompt_source_registry(self.prompt_source_registry.clone());
        agent.set_skill_source_registry(self.skill_source_registry.clone());
        agent.set_lsp_manager(self.lsp_manager.clone());
        agent.set_agent_tree_control(Some(self.agent_tree_control.clone()));
        if let Some(session_id) = self.session_id {
            agent.set_session_id(session_id);
        }
        if !self.initial_transcript.is_empty() {
            agent.replace_history(self.initial_transcript);
        }
        agent.set_transcript_persistence_enabled(self.transcript_persistence);
        agent.set_memory_facilities_enabled(self.memory_facilities);
        agent.set_hook_context(
            self.command_hook_registry,
            crate::hooks::HookSandbox {
                workspace_root: hook_workspace_root,
                ..crate::hooks::HookSandbox::default()
            },
            self.hook_runtime.clone(),
        );
        RuntimeSessionComponents {
            agent,
            warnings: self.warnings,
            sandbox_network_access: self.sandbox_network_access,
            goal_handle: self.goal_handle,
            mcp_tool_cache: self.mcp_tool_cache,
            mcp_manager: self.mcp_manager,
            prompt_source_registry: self.prompt_source_registry,
            skill_source_registry: self.skill_source_registry,
            hook_registry: self.hook_registry,
            hook_runtime: self.hook_runtime,
            lsp_manager: self.lsp_manager,
            event_bus: self.event_bus,
            memory_config,
            explicit_plugin_dirs,
        }
    }

    pub(crate) async fn into_session_components(self) -> RuntimeSessionComponents {
        let workspace_root = self.workspace.root.clone();
        let plugin_dirs = self.plugin_dirs.clone();
        self.into_session_components_for_plugin_dirs(workspace_root, plugin_dirs)
            .await
    }

    async fn into_session_components_for_plugin_dirs(
        self,
        workspace_root: PathBuf,
        plugin_dirs: Vec<PathBuf>,
    ) -> RuntimeSessionComponents {
        let rara_home = self.rara_home.clone();
        let builtin_plugins = self.builtin_plugins.clone();
        let extension_discovery = self.extension_discovery;
        let hook_runtime = self.hook_runtime.clone();
        let event_bus = self.event_bus.clone();
        let mut extension_readiness = self.extension_readiness.clone();
        let mut components = self.into_session_components_without_extensions(plugin_dirs.clone());
        if !extension_discovery {
            event_bus.publish_control(RuntimeEvent::Extension(ExtensionEvent::ReadinessUpdated {
                snapshot: extension_readiness,
            }));
            return components;
        }
        let plugin_hook_runtime = crate::plugin_middleware::register_plugin_hooks(
            &hook_runtime,
            rara_home,
            &workspace_root,
            &plugin_dirs,
            &builtin_plugins,
            &components.agent.session_id,
        )
        .await;
        extension_readiness.hook_count = plugin_hook_runtime.hook_count();
        extension_readiness.command_count = plugin_hook_runtime.command_summaries().len();
        components
            .agent
            .set_plugin_hook_runtime(plugin_hook_runtime);
        event_bus.publish_control(RuntimeEvent::Extension(ExtensionEvent::ReadinessUpdated {
            snapshot: extension_readiness,
        }));
        components
    }
}

pub(crate) struct RuntimeBootstrapOptions {
    pub plugin_dirs: Vec<PathBuf>,
    pub rara_home: Option<PathBuf>,
    pub agent_tree_config: AgentTreeConfig,
    pub agent_tree_control: Option<Arc<AgentTreeControl>>,
    pub backend: Option<Arc<dyn LlmBackend>>,
    pub tool_manager: Option<ToolManager>,
    pub extension_discovery: bool,
    pub session_id: Option<String>,
    pub initial_transcript: Vec<Message>,
    pub transcript_persistence: bool,
    pub memory_facilities: bool,
    pub session_profile: RuntimeSessionProfile,
    pub event_capacity: usize,
}

impl Default for RuntimeBootstrapOptions {
    fn default() -> Self {
        Self {
            plugin_dirs: Vec::new(),
            rara_home: None,
            agent_tree_config: AgentTreeConfig::default(),
            agent_tree_control: None,
            backend: None,
            tool_manager: None,
            extension_discovery: true,
            session_id: None,
            initial_transcript: Vec::new(),
            transcript_persistence: true,
            memory_facilities: true,
            session_profile: RuntimeSessionProfile::Default,
            event_capacity: 256,
        }
    }
}

impl RuntimeBootstrapOptions {
    pub(crate) fn with_plugin_dirs(plugin_dirs: Vec<PathBuf>) -> Self {
        Self {
            plugin_dirs,
            ..Self::default()
        }
    }

    pub(crate) fn with_rara_home(mut self, rara_home: Option<PathBuf>) -> Self {
        self.rara_home = rara_home;
        self
    }

    pub(crate) fn with_agent_tree_config(mut self, agent_tree_config: AgentTreeConfig) -> Self {
        self.agent_tree_config = agent_tree_config;
        self
    }

    pub(crate) fn with_agent_tree_control(
        mut self,
        agent_tree_control: Option<Arc<AgentTreeControl>>,
    ) -> Self {
        self.agent_tree_control = agent_tree_control;
        self
    }

    pub(crate) fn with_backend(mut self, backend: Option<Arc<dyn LlmBackend>>) -> Self {
        self.backend = backend;
        self
    }

    pub(crate) fn with_tool_manager(mut self, tool_manager: Option<ToolManager>) -> Self {
        self.tool_manager = tool_manager;
        self
    }

    pub(crate) fn with_extension_discovery(mut self, enabled: bool) -> Self {
        self.extension_discovery = enabled;
        self
    }

    pub(crate) fn with_session_id(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }

    pub(crate) fn with_initial_transcript(mut self, transcript: Vec<Message>) -> Self {
        self.initial_transcript = transcript;
        self
    }

    pub(crate) fn with_transcript_persistence(mut self, enabled: bool) -> Self {
        self.transcript_persistence = enabled;
        self
    }

    pub(crate) fn with_memory_facilities(mut self, enabled: bool) -> Self {
        self.memory_facilities = enabled;
        self
    }

    pub(crate) fn with_session_profile(mut self, profile: RuntimeSessionProfile) -> Self {
        self.session_profile = profile;
        self
    }

    pub(crate) fn with_event_capacity(mut self, capacity: usize) -> Self {
        self.event_capacity = capacity.max(1);
        self
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
    initialize_rara_context_with_options(
        config,
        workspace_root,
        progress,
        RuntimeBootstrapOptions::default(),
    )
    .await
}

pub(crate) async fn initialize_rara_context_for_workspace_with_options(
    config: &RaraConfig,
    workspace_root: Option<&Path>,
    progress: Option<LocalProgressReporter>,
    options: RuntimeBootstrapOptions,
) -> Result<RuntimeBootstrap> {
    initialize_rara_context_with_options(config, workspace_root, progress, options).await
}

pub(crate) async fn initialize_rara_context_with_options(
    config: &RaraConfig,
    workspace_root: Option<&Path>,
    progress: Option<LocalProgressReporter>,
    mut options: RuntimeBootstrapOptions,
) -> Result<RuntimeBootstrap> {
    let session_profile = options.session_profile;
    if session_profile.disables_ambient_facilities() {
        options.extension_discovery = false;
        options.transcript_persistence = false;
        options.memory_facilities = false;
    }
    let multi_agent_policy = match session_profile {
        RuntimeSessionProfile::Default => config.multi_agent_policy,
        RuntimeSessionProfile::HeadlessCodingV1 => MultiAgentPolicy::Disabled,
    };
    let rara_home = match options.rara_home.clone() {
        Some(rara_home) => {
            std::fs::create_dir_all(&rara_home)?;
            rara_home
        }
        None => ensure_rara_home_dir()?,
    };
    let backend = match options.backend.take() {
        Some(backend) => backend,
        None => {
            let chat_backend =
                build_backend_with_progress_for_home(config, progress, Some(&rara_home)).await?;
            chat_backend.into()
        }
    };

    let workspace = match workspace_root {
        Some(root) => Arc::new(WorkspaceMemory::from_paths(
            root.to_path_buf(),
            rara_config::workspace_data_dir_for_home(root, &rara_home)?,
        )),
        None => Arc::new(WorkspaceMemory::new()?),
    };
    let memory_handle = Arc::new(MemoryHandle::new(&memory_handle_uri_for_workspace(
        &workspace,
    )));
    let session_manager = Arc::new(SessionManager::new_for_rara_dir(
        workspace.rara_dir.clone(),
    )?);
    let shell_env = capture_shell_environment_snapshot().await;
    let sandbox_manager = Arc::new(SandboxManager::new_with_command_path(
        shell_env.env.get("PATH").cloned(),
    )?);

    let config_manager = crate::config::ConfigManager::new_for_rara_home(rara_home.clone())?;
    let plugins = if options.extension_discovery {
        crate::plugin_middleware::discover_runtime_plugins(
            Some(&rara_home),
            &workspace.root,
            &options.plugin_dirs,
            &config.builtin_plugins,
        )
    } else {
        Vec::new()
    };
    let plugin_skill_roots = crate::plugin_middleware::plugin_skill_roots(&plugins);
    let plugin_agent_records = crate::plugin_middleware::plugin_agent_records(&plugins);

    let mut prompt_config = PromptRuntimeConfig::from_config(config);
    if let Some(system_prompt) = session_profile.system_prompt() {
        prompt_config.system_prompt = Some(system_prompt.to_string());
        prompt_config.append_system_prompt = None;
        prompt_config.compact_prompt = None;
        prompt_config.context_file_search = rara_config::ContextFileSearchPolicy::Off;
    }
    if options.extension_discovery {
        append_builtin_prompt_instructions(&mut prompt_config, &config.builtin_plugins);
    }
    append_multi_agent_prompt_instructions(&mut prompt_config, multi_agent_policy);
    let skill_manager = if options.extension_discovery {
        load_skill_manager(&mut prompt_config.warnings, &plugin_skill_roots)
    } else {
        Arc::new(RwLock::new(SkillManager::new()))
    };
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

    let event_bus = Arc::new(RuntimeEventBus::new(options.event_capacity));
    let hook_runtime = Arc::new(HookRuntime::new(event_bus.clone()));
    hook_runtime.start();
    let prompt_source_registry = Arc::new(PromptSourceRegistry::new(event_bus.clone()));
    let skill_source_registry = Arc::new(SkillSourceRegistry::new(event_bus.clone()));
    let hook_registry = Arc::new(HookRegistry::new(event_bus.clone()));
    let goal_handle: GoalHandle = Arc::new(std::sync::RwLock::new(None));
    let mcp_tool_cache = McpToolCache::new();
    mcp_tool_cache.clear();
    let lsp_manager = Arc::new(LspManager::new(workspace.root.clone()));

    let mut mcp_registry = if options.extension_discovery {
        config_manager
            .load_mcp_registry_for_project(&workspace.root)
            .unwrap_or_else(|_| crate::config::McpRegistry::empty())
    } else {
        crate::config::McpRegistry::empty()
    };
    if options.extension_discovery {
        crate::plugin_middleware::append_plugin_mcp_configs(
            &mut mcp_registry,
            Some(&rara_home),
            &workspace.root,
            &options.plugin_dirs,
            &config.builtin_plugins,
        )?;
    }
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
    if options.extension_discovery {
        file_hooks.discover_repo_hooks(&workspace.root);
    }
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
    let agent_definitions = if options.extension_discovery {
        AgentDefinitionCache::load_with_records(workspace.root.clone(), plugin_agent_records)
    } else {
        AgentDefinitionCache::empty()
    };
    let subagent_backend_resolver: Arc<dyn SubagentBackendResolver> =
        Arc::new(ConfigSubagentBackendResolver::new_for_rara_home(
            Arc::new(config.clone()),
            rara_home.clone(),
        ));
    let agent_tree_control = options
        .agent_tree_control
        .take()
        .unwrap_or_else(|| Arc::new(AgentTreeControl::new(options.agent_tree_config)));
    let mut tool_manager = options.tool_manager.take().unwrap_or_else(|| {
        create_full_tool_manager(
            backend.clone(),
            memory_handle.clone(),
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
            agent_tree_control.clone(),
            multi_agent_policy,
            subagent_backend_resolver.clone(),
            agent_definitions.clone(),
        )
    });
    session_profile.project_tools(&mut tool_manager);
    let mut warnings = prompt_config.warnings.clone();
    warnings.extend(file_hook_warnings);

    Ok(RuntimeBootstrap {
        backend,
        memory_handle,
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
        agent_tree_control,
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
        extension_discovery: options.extension_discovery,
        session_id: options.session_id,
        initial_transcript: options.initial_transcript,
        transcript_persistence: options.transcript_persistence,
        memory_facilities: options.memory_facilities,
    })
}

fn append_builtin_prompt_instructions(
    prompt_config: &mut PromptRuntimeConfig,
    builtin_plugins: &BuiltinPluginConfig,
) {
    let Some(memory_instructions) =
        crate::plugin_middleware::nowledge_mem_prompt_instructions(builtin_plugins)
    else {
        return;
    };
    prompt_config.append_system_prompt = Some(match prompt_config.append_system_prompt.take() {
        Some(existing) => format!("{existing}\n\n{memory_instructions}"),
        None => memory_instructions.to_string(),
    });
}

fn append_multi_agent_prompt_instructions(
    prompt_config: &mut PromptRuntimeConfig,
    policy: MultiAgentPolicy,
) {
    let instructions = match policy {
        MultiAgentPolicy::Disabled => return,
        MultiAgentPolicy::Explicit => concat!(
            "## Multi-Agent Policy\n",
            "- Delegation is explicit-only for this runtime.\n",
            "- Use agent tools only when the user requests delegation or the task contract explicitly requires parallel agent work.\n",
            "- A higher reasoning effort does not enable proactive delegation.\n",
            "- When delegating, assign non-overlapping tasks and synthesize the returned evidence yourself."
        ),
        MultiAgentPolicy::ProactiveReadOnly => concat!(
            "## Multi-Agent Policy\n",
            "- You may proactively delegate bounded independent research, repository exploration, planning, or review when parallel work materially improves the result.\n",
            "- Proactive work must use read-only roles such as explore, plan, code-reviewer, architect, or researcher. Do not proactively select a general or custom mutation-capable agent.\n",
            "- Do not delegate trivial work, duplicate a child's assignment, or use delegation as a substitute for synthesis.\n",
            "- Launch independent tasks together when practical, continue non-overlapping work, and use wait_agent when their results are required.\n",
            "- Model selection and permissions are independent: choose an appropriate child provider/model without weakening the selected role's tool policy."
        ),
    };
    prompt_config.append_system_prompt = Some(match prompt_config.append_system_prompt.take() {
        Some(existing) => format!("{existing}\n\n{instructions}"),
        None => instructions.to_string(),
    });
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

#[cfg(test)]
pub(crate) async fn build_backend_with_progress(
    config: &RaraConfig,
    progress: Option<LocalProgressReporter>,
) -> Result<Box<dyn LlmBackend>> {
    build_backend_with_progress_for_home(config, progress, None).await
}

async fn build_backend_with_progress_for_home(
    config: &RaraConfig,
    progress: Option<LocalProgressReporter>,
    rara_home: Option<&Path>,
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
                    "kimi-coding" => OpenAiEndpointKind::KimiCoding,
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
            let home = match rara_home {
                Some(home) => home.to_path_buf(),
                None => ensure_rara_home_dir()?,
            };
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

fn memory_handle_uri_for_workspace(workspace: &WorkspaceMemory) -> String {
    workspace.rara_dir.join("memory").display().to_string()
}

#[cfg(test)]
mod tests;
