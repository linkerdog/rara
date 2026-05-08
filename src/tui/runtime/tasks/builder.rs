use crate::tui::state::RebuildSuccess;

pub(super) async fn rebuild_agent_with_progress(
    config: &crate::config::RaraConfig,
    progress: Option<crate::local_backend::LocalProgressReporter>,
) -> anyhow::Result<RebuildSuccess> {
    let bootstrap = crate::runtime_context::initialize_rara_context(config, progress).await?;
    let (agent, warnings, sandbox_network_access, goal_handle, mcp_tool_cache) =
        bootstrap.into_parts();
    Ok(RebuildSuccess {
        agent,
        warnings,
        sandbox_network_access,
        goal_handle,
        mcp_tool_cache,
    })
}
