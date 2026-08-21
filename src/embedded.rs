//! Supported Rust embedding facade for the RARA runtime.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::broadcast;

use crate::agent::{Agent, AgentEvent, AgentOutputMode};
use crate::config::RaraConfig;
use crate::model_observation::QueryReport;
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};
use crate::runtime_control::RuntimeControlEvent;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tools::agent::{
    AgentMailboxMessage, AgentSnapshot, AgentTreeConfig, AgentTreeControl, AgentWaitResult,
};

/// Construction options for one embedded runtime instance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmbeddedRuntimeOptions {
    /// Additional plugin roots supplied by the embedding application.
    pub plugin_dirs: Vec<PathBuf>,
    /// Optional RARA state root. Supplying it scopes sessions, configuration,
    /// workspace data, and provider-owned credential storage for this runtime.
    pub state_root: Option<PathBuf>,
    /// Session-tree concurrency policy. The default permits three active
    /// children in addition to the root agent.
    pub agent_tree_config: AgentTreeConfig,
}

/// One workspace-scoped RARA runtime that can be embedded in another Rust
/// application without routing through the CLI or TUI.
pub struct EmbeddedRuntime {
    agent: Agent,
    workspace_root: PathBuf,
    event_bus: Arc<RuntimeEventBus>,
    agent_tree_control: Arc<AgentTreeControl>,
}

impl EmbeddedRuntime {
    /// Construct an embedded runtime with default bootstrap options.
    pub async fn from_config(
        config: &RaraConfig,
        workspace_root: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::from_config_with_options(config, workspace_root, EmbeddedRuntimeOptions::default())
            .await
    }

    /// Construct an embedded runtime for an explicit workspace and state scope.
    pub async fn from_config_with_options(
        config: &RaraConfig,
        workspace_root: impl AsRef<Path>,
        options: EmbeddedRuntimeOptions,
    ) -> Result<Self> {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let bootstrap = initialize_rara_context_for_workspace_with_options(
            config,
            Some(&workspace_root),
            None,
            RuntimeBootstrapOptions::with_plugin_dirs(options.plugin_dirs)
                .with_rara_home(options.state_root)
                .with_agent_tree_config(options.agent_tree_config),
        )
        .await?;
        let event_bus = bootstrap.event_bus.clone();
        let agent_tree_control = bootstrap.agent_tree_control.clone();
        let agent = bootstrap.into_agent().await;
        Ok(Self {
            agent,
            workspace_root,
            event_bus,
            agent_tree_control,
        })
    }

    /// Return the root runtime session id.
    pub fn session_id(&self) -> &str {
        &self.agent.session_id
    }

    /// Return the workspace owned by this runtime instance.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Clone the opaque session-tree control handle.
    pub fn agent_tree_control(&self) -> Arc<AgentTreeControl> {
        self.agent_tree_control.clone()
    }

    /// List children owned by this runtime's root session.
    pub fn list_agents(&self) -> Result<Vec<AgentSnapshot>> {
        let mut agents = self
            .agent_tree_control
            .snapshots_for_parent(self.session_id())?;
        agents.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(agents)
    }

    /// Wait for mailbox activity from any child or the supplied ids/paths.
    pub async fn wait_for_agents(
        &self,
        targets: &[String],
        timeout: Duration,
    ) -> Result<AgentWaitResult> {
        let targets = if targets.is_empty() {
            None
        } else {
            Some(
                targets
                    .iter()
                    .map(|target| {
                        self.agent_tree_control
                            .resolve_agent_id_for_parent(target, self.session_id())
                    })
                    .collect::<Result<HashSet<_>, _>>()?,
            )
        };
        let (messages, timed_out) = self
            .agent_tree_control
            .wait_for_messages(self.session_id(), targets.as_ref(), timeout)
            .await?;
        Ok(AgentWaitResult {
            messages,
            timed_out,
        })
    }

    /// Send a message that the child will receive at its next model boundary.
    pub fn send_agent_message(
        &self,
        target: &str,
        message: impl Into<String>,
    ) -> Result<AgentMailboxMessage> {
        Ok(self.agent_tree_control.send_to_child(
            self.session_id(),
            target,
            "message",
            message.into(),
        )?)
    }

    /// Send a follow-up instruction to a currently running child.
    pub fn followup_agent_task(
        &self,
        target: &str,
        instruction: impl Into<String>,
    ) -> Result<AgentMailboxMessage> {
        Ok(self.agent_tree_control.send_to_child(
            self.session_id(),
            target,
            "followup",
            instruction.into(),
        )?)
    }

    /// Signal cancellation for a child owned by this runtime.
    pub fn interrupt_agent(&self, target: &str) -> Result<AgentSnapshot> {
        Ok(self
            .agent_tree_control
            .interrupt_for_parent(target, self.session_id())?)
    }

    /// Subscribe to typed control-plane events for this runtime.
    pub fn subscribe_control(&self) -> broadcast::Receiver<RuntimeControlEvent> {
        self.event_bus.subscribe_control()
    }

    /// Execute one prompt and report typed agent events to the callback.
    pub async fn query_with_events<F>(
        &mut self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.agent
            .query_with_mode_and_events(prompt.into(), output_mode, report)
            .await
    }

    /// Execute one prompt and return structured per-request model observations.
    ///
    /// The returned report contains token accounting, duration, and optional
    /// SHA-256 request fingerprints. It never contains prompt or response text.
    pub async fn query_with_report<F>(
        &mut self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        report: F,
    ) -> Result<QueryReport>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.agent
            .query_with_mode_and_events(prompt.into(), output_mode, report)
            .await?;
        Ok(self.agent.last_query_report.clone())
    }

    pub(crate) fn replace_llm_backend(&mut self, backend: Arc<dyn crate::llm::LlmBackend>) {
        self.agent.llm_backend = backend;
    }

    pub(crate) fn set_max_turns(&mut self, max_turns: usize) {
        self.agent.set_max_turns(max_turns);
    }

    pub(crate) fn disable_tools(&mut self) {
        self.agent.tool_manager.retain(|_| false);
    }

    pub(crate) fn disable_extension_execution(&mut self) {
        self.agent.disable_extension_execution();
    }
}
