use std::path::PathBuf;
use std::sync::Arc;

use rara_tools::tool::ToolManager;

use crate::llm::{LlmBackend, Message};
use crate::runtime_session::RuntimeSessionProfile;
use crate::tools::agent::{AgentTreeConfig, AgentTreeControl};

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
    pub replay_capacity: usize,
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
            replay_capacity: 1024,
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
        self.replay_capacity = self.event_capacity;
        self
    }
}
