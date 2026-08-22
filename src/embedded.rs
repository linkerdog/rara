//! Supported Rust embedding facade for the RARA runtime.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::broadcast;

use crate::agent::{AgentEvent, AgentOutputMode};
use crate::config::RaraConfig;
use crate::llm::LlmBackend;
use crate::model_observation::QueryReport;
use crate::runtime_control::RuntimeControlEvent;
use crate::runtime_session::RuntimeSession;
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
    session: RuntimeSession,
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
        let mut builder = RuntimeSession::builder(config.clone(), &workspace_root)
            .with_plugin_dirs(options.plugin_dirs)
            .with_agent_tree_config(options.agent_tree_config);
        if let Some(state_root) = options.state_root {
            builder = builder.with_state_root(state_root);
        }
        let session = builder.build().await?;
        Ok(Self { session })
    }

    /// Return the root runtime session id.
    pub fn session_id(&self) -> &str {
        self.session.id().as_str()
    }

    /// Return the workspace owned by this runtime instance.
    pub fn workspace_root(&self) -> &Path {
        self.session.workspace_root()
    }

    /// Clone the opaque session-tree control handle.
    pub fn agent_tree_control(&self) -> Arc<AgentTreeControl> {
        self.session.agent_tree_control()
    }

    /// List children owned by this runtime's root session.
    pub fn list_agents(&self) -> Result<Vec<AgentSnapshot>> {
        let mut agents = self
            .session
            .agent_tree_control()
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
                        self.session
                            .agent_tree_control()
                            .resolve_agent_id_for_parent(target, self.session_id())
                    })
                    .collect::<Result<HashSet<_>, _>>()?,
            )
        };
        let (messages, timed_out) = self
            .session
            .agent_tree_control()
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
        Ok(self.session.agent_tree_control().send_to_child(
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
        Ok(self.session.agent_tree_control().send_to_child(
            self.session_id(),
            target,
            "followup",
            instruction.into(),
        )?)
    }

    /// Signal cancellation for a child owned by this runtime.
    pub fn interrupt_agent(&self, target: &str) -> Result<AgentSnapshot> {
        Ok(self
            .session
            .agent_tree_control()
            .interrupt_for_parent(target, self.session_id())?)
    }

    /// Subscribe to typed control-plane events for this runtime.
    pub fn subscribe_control(&self) -> broadcast::Receiver<RuntimeControlEvent> {
        self.session.subscribe_control()
    }

    /// Clone the canonical session handle used by this compatibility facade.
    pub fn runtime_session(&self) -> RuntimeSession {
        self.session.clone()
    }

    /// Execute one prompt and report typed agent events to the callback.
    pub async fn query_with_events<F>(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.session
            .query_with_events(prompt, output_mode, report)
            .await?;
        Ok(())
    }

    /// Execute one prompt and return structured per-request model observations.
    ///
    /// The returned report contains token accounting, duration, and optional
    /// SHA-256 request fingerprints. It never contains prompt or response text.
    pub async fn query_with_report<F>(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        report: F,
    ) -> Result<QueryReport>
    where
        F: FnMut(AgentEvent) + Send,
    {
        Ok(self
            .session
            .query_with_report(prompt, output_mode, report)
            .await?)
    }

    pub(crate) async fn replace_llm_backend(&self, backend: Arc<dyn LlmBackend>) -> Result<()> {
        self.session.replace_llm_backend(backend).await?;
        Ok(())
    }

    pub(crate) async fn set_max_turns(&self, max_turns: usize) -> Result<()> {
        self.session.set_max_turns(max_turns).await?;
        Ok(())
    }

    pub(crate) async fn disable_tools(&self) -> Result<()> {
        self.session.disable_tools().await?;
        Ok(())
    }

    pub(crate) async fn disable_extension_execution(&self) -> Result<()> {
        self.session.disable_extension_execution().await?;
        Ok(())
    }

    /// Explicitly drain and stop the canonical session actor.
    pub async fn shutdown(&self) -> Result<()> {
        self.session.shutdown().await?;
        Ok(())
    }
}
