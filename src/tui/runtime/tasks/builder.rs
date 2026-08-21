use std::sync::Arc;

use crate::runtime_client::RebuildSuccess;

pub(super) async fn rebuild_agent_with_progress(
    config: &crate::config::RaraConfig,
    progress: Option<crate::local_backend::LocalProgressReporter>,
    plugin_dirs: Vec<std::path::PathBuf>,
    agent_tree_control: Option<Arc<crate::tools::agent::AgentTreeControl>>,
) -> anyhow::Result<RebuildSuccess> {
    let bootstrap = crate::runtime_context::initialize_rara_context_for_workspace_with_options(
        config,
        None,
        progress,
        crate::runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(plugin_dirs)
            .with_agent_tree_control(agent_tree_control),
    )
    .await?;
    let components = bootstrap.into_session_components().await;
    let event_bus = components.event_bus;
    let memory_handler = Arc::new(crate::protocol_sources::MemoryControlHandler::with_store(
        event_bus,
        components.agent.memory_store.clone(),
    ));
    Ok(RebuildSuccess {
        agent: components.agent,
        warnings: components.warnings,
        sandbox_network_access: components.sandbox_network_access,
        goal_handle: components.goal_handle,
        mcp_tool_cache: components.mcp_tool_cache,
        mcp_manager: components.mcp_manager,
        prompt_source_registry: components.prompt_source_registry,
        skill_source_registry: components.skill_source_registry,
        hook_registry: components.hook_registry,
        hook_runtime: components.hook_runtime,
        memory_handler,
        lsp_manager: components.lsp_manager,
    })
}
