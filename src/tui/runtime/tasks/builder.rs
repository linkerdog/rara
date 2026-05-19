use std::sync::Arc;

use crate::tui::state::RebuildSuccess;

pub(super) async fn rebuild_agent_with_progress(
    config: &crate::config::RaraConfig,
    progress: Option<crate::local_backend::LocalProgressReporter>,
) -> anyhow::Result<RebuildSuccess> {
    let bootstrap = crate::runtime_context::initialize_rara_context(config, progress).await?;
    let local_model_server = crate::local_model_server::inspect_local_model_server_status(
        &crate::config::ensure_rara_home_dir()?,
    );
    let event_bus = bootstrap.event_bus.clone();

    // Start the hook runtime — registers command-type hooks found in `.claude/hooks/`
    let hook_runtime = crate::hook_runtime::HookRuntime::new(event_bus.clone());
    hook_runtime.start();

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
        local_model_server,
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
