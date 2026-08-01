//! Session-scoped runtime ownership for presentation surfaces.
//!
//! A runtime client owns the execution objects for one session. Presentation
//! surfaces may retain this client and submit commands through its API, but
//! they must not construct or own the underlying registries independently.

use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

use crate::agent::Agent;
use crate::hook_registry::HookRegistry;
use crate::hook_runtime::HookRuntime;
use crate::lsp_manager::LspManager;
use crate::mcp_connection_manager::McpConnectionManager;
use crate::mcp_tool_cache::McpToolCache;
use crate::protocol_sources::{PromptSourceRegistry, SkillSourceRegistry};
use crate::runtime_context::RuntimeBootstrap;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tui::state::GoalHandle;

/// Runtime objects owned by one interactive session.
pub(crate) struct RuntimeClient {
    agent: Option<Agent>,
    pub(crate) goal_handle: GoalHandle,
    pub(crate) mcp_tool_cache: McpToolCache,
    pub(crate) mcp_manager: Arc<McpConnectionManager>,
    pub(crate) prompt_source_registry: Arc<PromptSourceRegistry>,
    pub(crate) skill_source_registry: Arc<SkillSourceRegistry>,
    pub(crate) hook_registry: Arc<HookRegistry>,
    pub(crate) hook_runtime: Arc<HookRuntime>,
    pub(crate) lsp_manager: Arc<LspManager>,
    pub(crate) sandbox_network_access: Arc<AtomicBool>,
    pub(crate) event_bus: Arc<RuntimeEventBus>,
    pub(crate) explicit_plugin_dirs: Vec<PathBuf>,
}

impl RuntimeClient {
    /// Convert a fully bootstrapped runtime into a session-owned client.
    pub(crate) async fn from_bootstrap(bootstrap: RuntimeBootstrap) -> Self {
        let event_bus = bootstrap.event_bus.clone();
        let (
            (
                agent,
                _warnings,
                sandbox_network_access,
                goal_handle,
                mcp_tool_cache,
                mcp_manager,
                prompt_source_registry,
                skill_source_registry,
                hook_registry,
                hook_runtime,
                lsp_manager,
            ),
            explicit_plugin_dirs,
        ) = bootstrap.into_runtime_client_parts().await;
        Self {
            agent: Some(agent),
            goal_handle,
            mcp_tool_cache,
            mcp_manager,
            prompt_source_registry,
            skill_source_registry,
            hook_registry,
            hook_runtime,
            lsp_manager,
            sandbox_network_access,
            event_bus,
            explicit_plugin_dirs,
        }
    }

    pub(crate) fn agent(&self) -> Option<&Agent> {
        self.agent.as_ref()
    }

    pub(crate) fn agent_mut(&mut self) -> &mut Option<Agent> {
        &mut self.agent
    }
}
