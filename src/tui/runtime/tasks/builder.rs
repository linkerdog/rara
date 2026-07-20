use std::sync::Arc;

use crate::tui::state::RebuildSuccess;

pub(super) async fn rebuild_agent_with_progress(
    config: &crate::config::RaraConfig,
    progress: Option<crate::local_backend::LocalProgressReporter>,
    plugin_dirs: Vec<std::path::PathBuf>,
) -> anyhow::Result<RebuildSuccess> {
    let bootstrap = crate::runtime_context::initialize_rara_context_for_workspace_with_options(
        config,
        None,
        progress,
        crate::runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(plugin_dirs),
    )
    .await?;
    // `inspect_local_model_server_status` uses a `reqwest::blocking` client, which spins up and
    // drops its own Tokio runtime; dropping a runtime inside the async context would panic, so run
    // it on a blocking thread where that is allowed.
    let rara_home = crate::config::ensure_rara_home_dir()?;
    let local_model_server = tokio::task::spawn_blocking(move || {
        crate::local_model_server::inspect_local_model_server_status(&rara_home)
    })
    .await?;
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
        hook_runtime,
        lsp_manager,
    ) = bootstrap.into_parts_with_runtime_extensions().await;
    let memory_handler = Arc::new(crate::protocol_sources::MemoryControlHandler::with_store(
        event_bus,
        agent.memory_store.clone(),
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
        hook_runtime,
        memory_handler,
        lsp_manager,
    })
}
