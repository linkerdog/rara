use std::sync::Arc;

use crate::tui::state::RebuildSuccess;

pub(super) async fn rebuild_agent_with_progress(
    config: &crate::config::RaraConfig,
    progress: Option<crate::local_backend::LocalProgressReporter>,
) -> anyhow::Result<RebuildSuccess> {
    let bootstrap = crate::runtime_context::initialize_rara_context(config, progress).await?;
    let event_bus = bootstrap.event_bus.clone();
    let (
        agent,
        warnings,
        sandbox_network_access,
        goal_handle,
        mcp_tool_cache,
        mcp_manager,
        prompt_source_registry,
        skill_source_registry,
        hook_registry,
    ) = bootstrap.into_parts();
    let memory_handler = Arc::new(crate::protocol_sources::MemoryControlHandler::new(
        event_bus,
    ));
    Ok(RebuildSuccess {
        agent,
        warnings,
        sandbox_network_access,
        goal_handle,
        mcp_tool_cache,
        mcp_manager,
        prompt_source_registry,
        skill_source_registry,
        hook_registry,
        memory_handler,
    })
}
