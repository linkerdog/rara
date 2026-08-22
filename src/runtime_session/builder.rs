use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use rara_tools::tool::ToolManager;

use super::{RuntimeSession, RuntimeSessionProfile};
use crate::config::RaraConfig;
use crate::llm::{LlmBackend, Message};
use crate::runtime_client::RuntimeClient;
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};
use crate::tools::agent::AgentTreeConfig;

pub(super) const DEFAULT_COMMAND_CAPACITY: usize = 32;

/// Assembles one `RuntimeSession` from application defaults or host components.
pub struct RuntimeSessionBuilder {
    config: RaraConfig,
    workspace_root: PathBuf,
    plugin_dirs: Vec<PathBuf>,
    state_root: Option<PathBuf>,
    agent_tree_config: AgentTreeConfig,
    backend: Option<Arc<dyn LlmBackend>>,
    tool_manager: Option<ToolManager>,
    command_capacity: usize,
    event_capacity: usize,
    session_id: Option<String>,
    initial_transcript: Vec<Message>,
    persist_transcript: bool,
    enable_memory_facilities: bool,
    enable_extension_discovery: bool,
    require_state_root: bool,
    profile: RuntimeSessionProfile,
}

impl RuntimeSessionBuilder {
    /// Create a builder for an explicit workspace without changing process cwd.
    pub fn new(config: RaraConfig, workspace_root: impl AsRef<Path>) -> Self {
        Self {
            config,
            workspace_root: workspace_root.as_ref().to_path_buf(),
            plugin_dirs: Vec::new(),
            state_root: None,
            agent_tree_config: AgentTreeConfig::default(),
            backend: None,
            tool_manager: None,
            command_capacity: DEFAULT_COMMAND_CAPACITY,
            event_capacity: 256,
            session_id: None,
            initial_transcript: Vec::new(),
            persist_transcript: true,
            enable_memory_facilities: true,
            enable_extension_discovery: true,
            require_state_root: false,
            profile: RuntimeSessionProfile::Default,
        }
    }

    /// Create a host-controlled builder with explicit backend and tools.
    ///
    /// This path requires `with_state_root` and disables ambient extensions,
    /// RARA-owned memory, and transcript checkpoints so the embedding
    /// application remains the authority for those facilities.
    pub fn for_host(
        config: RaraConfig,
        workspace_root: impl AsRef<Path>,
        backend: Arc<dyn LlmBackend>,
        tool_manager: ToolManager,
    ) -> Self {
        let mut builder = Self::new(config, workspace_root)
            .with_backend(backend)
            .with_tool_manager(tool_manager)
            .without_extension_discovery()
            .without_memory_facilities()
            .without_transcript_persistence();
        builder.require_state_root = true;
        builder
    }

    /// Add explicit plugin roots to application-owned runtime discovery.
    pub fn with_plugin_dirs(mut self, plugin_dirs: Vec<PathBuf>) -> Self {
        self.plugin_dirs = plugin_dirs;
        self
    }

    /// Scope RARA state and provider-owned credentials to an explicit root.
    pub fn with_state_root(mut self, state_root: impl Into<PathBuf>) -> Self {
        self.state_root = Some(state_root.into());
        self
    }

    /// Set the child-agent concurrency policy for this session.
    pub fn with_agent_tree_config(mut self, config: AgentTreeConfig) -> Self {
        self.agent_tree_config = config;
        self
    }

    /// Inject a host-owned model backend instead of constructing a provider.
    pub fn with_backend(mut self, backend: Arc<dyn LlmBackend>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Inject the exact tool registry exposed to this session.
    pub fn with_tool_manager(mut self, tool_manager: ToolManager) -> Self {
        self.tool_manager = Some(tool_manager);
        self
    }

    /// Use a stable host-owned session identity.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Select a stable runtime composition for this session.
    ///
    /// Versioned profiles override ambient extension, memory, transcript, and
    /// tool-registry settings at build time so call order cannot widen them.
    pub fn with_profile(mut self, profile: RuntimeSessionProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Hydrate the model-visible transcript before the first submitted turn.
    pub fn with_transcript(mut self, transcript: Vec<Message>) -> Self {
        self.initial_transcript = transcript;
        self
    }

    /// Disable local transcript, context, and compaction checkpoints.
    pub fn without_transcript_persistence(mut self) -> Self {
        self.persist_transcript = false;
        self
    }

    /// Explicitly opt a host back into local transcript-derived checkpoints.
    pub fn with_transcript_persistence(mut self) -> Self {
        self.persist_transcript = true;
        self
    }

    /// Disable RARA memory retrieval, consolidation, and built-in capture.
    pub fn without_memory_facilities(mut self) -> Self {
        self.enable_memory_facilities = false;
        self
    }

    /// Explicitly opt a host into configured RARA memory facilities.
    pub fn with_memory_facilities(mut self) -> Self {
        self.enable_memory_facilities = true;
        self
    }

    /// Disable ambient plugins, hooks, skills, agents, and MCP discovery.
    pub fn without_extension_discovery(mut self) -> Self {
        self.enable_extension_discovery = false;
        self
    }

    /// Explicitly opt a host into configured extension discovery.
    pub fn with_extension_discovery(mut self) -> Self {
        self.enable_extension_discovery = true;
        self
    }

    /// Override the bounded session command capacity.
    pub fn with_command_capacity(mut self, command_capacity: usize) -> Self {
        self.command_capacity = command_capacity.max(1);
        self
    }

    /// Override the bounded raw, structured, and replay event capacity.
    pub fn with_event_capacity(mut self, event_capacity: usize) -> Self {
        self.event_capacity = event_capacity.max(1);
        self
    }

    /// Assemble and start the session actor.
    pub async fn build(mut self) -> Result<RuntimeSession> {
        if self
            .session_id
            .as_deref()
            .is_some_and(|session_id| session_id.trim().is_empty())
        {
            anyhow::bail!("runtime session id must not be empty");
        }
        if self.require_state_root && self.state_root.is_none() {
            anyhow::bail!("host runtime requires an explicit state root");
        }
        if self.profile.disables_ambient_facilities() {
            self.enable_extension_discovery = false;
            self.enable_memory_facilities = false;
            self.persist_transcript = false;
        }
        if !self.enable_memory_facilities {
            self.config.builtin_plugins.nowledge_mem.enabled = false;
        }
        let options = RuntimeBootstrapOptions::with_plugin_dirs(self.plugin_dirs)
            .with_rara_home(self.state_root)
            .with_agent_tree_config(self.agent_tree_config)
            .with_backend(self.backend)
            .with_tool_manager(self.tool_manager)
            .with_extension_discovery(self.enable_extension_discovery)
            .with_session_id(self.session_id)
            .with_initial_transcript(self.initial_transcript)
            .with_transcript_persistence(self.persist_transcript)
            .with_memory_facilities(self.enable_memory_facilities)
            .with_session_profile(self.profile)
            .with_event_capacity(self.event_capacity);
        let bootstrap = initialize_rara_context_for_workspace_with_options(
            &self.config,
            Some(&self.workspace_root),
            None,
            options,
        )
        .await?;
        let client = RuntimeClient::from_bootstrap(bootstrap).await;
        RuntimeSession::start(client, self.command_capacity)
    }
}
