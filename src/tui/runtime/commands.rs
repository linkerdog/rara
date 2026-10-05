use std::path::PathBuf;
use std::sync::Arc;

use super::super::state::{
    HelpTab, ListPickerKind, LocalCommand, LocalCommandKind, Overlay, PermissionMode, RuntimePhase,
    StatusTab, SystemMessageKind, TuiApp,
};
#[cfg(test)]
use super::goals::{
    parse_goal_objective_and_budget, parse_goal_token_budget, start_goal_follow_up,
};
use super::tasks::{start_compact_task, start_rebuild_task};
use crate::agent::{Agent, AgentEvent, AgentExecutionMode, BashApprovalMode};
use crate::config::{McpRegistry, SourcedMcpServerConfig};
use crate::mcp_status::{McpStatusSnapshot, format_mcp_status};
use crate::mcp_tool_cache::McpToolCache;
use crate::oauth::OAuthManager;
use crate::runtime_control::RuntimeProvenance;
use crate::tui::runtime_port::{RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand};
use crate::tui::state::NoticeLevel;

pub(super) async fn execute_local_command(
    command: LocalCommand,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    _oauth_manager: &Arc<OAuthManager>,
) -> anyhow::Result<bool> {
    execute_local_command_with_runtime(command, app, agent_slot, None).await
}

pub(super) async fn execute_local_command_with_runtime(
    command: LocalCommand,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<bool> {
    if let Some(reason) = crate::tui::command::command_unavailable_reason(app, &command) {
        app.push_notice(NoticeLevel::Warning, reason);
        return Ok(false);
    }
    let command_kind = command.kind;
    app.remember_command(match command.kind {
        LocalCommandKind::Approval => "approval",
        LocalCommandKind::Clear => "clear",
        LocalCommandKind::Compact => "compact",
        LocalCommandKind::Connect => "connect",
        LocalCommandKind::Context => "context",
        LocalCommandKind::Help => "help",
        LocalCommandKind::Mcp => "mcp",
        LocalCommandKind::Model => "model",
        LocalCommandKind::NowledgeMem => "mem",
        LocalCommandKind::Plan => "plan",
        LocalCommandKind::Quit => "quit",
        LocalCommandKind::Resume => "resume",
        LocalCommandKind::Review => "review",
        LocalCommandKind::Status => "status",
        LocalCommandKind::Tasks => "tasks",
        LocalCommandKind::Skills => "skills",
        LocalCommandKind::Permissions => "permissions",
        LocalCommandKind::Goal => "goal",
    });
    match command.kind {
        LocalCommandKind::Approval => {
            if app.is_busy() {
                app.push_notice(
                    NoticeLevel::Info,
                    "A task is already running. Wait for it to finish.",
                );
                return Ok(false);
            }
            if app.permission_mode == PermissionMode::FullAccess {
                app.push_notice(
                    NoticeLevel::Info,
                    "Full Access already allows bash. Use /permissions to change the profile.",
                );
                return Ok(false);
            }
            let next_mode = match app.bash_approval_mode {
                BashApprovalMode::Suggestion => BashApprovalMode::Always,
                BashApprovalMode::Once => BashApprovalMode::Suggestion,
                BashApprovalMode::Always => BashApprovalMode::Suggestion,
            };
            app.bash_approval_mode = next_mode;
            app.permission_mode = PermissionMode::Custom;
            if let Some(agent) = agent_slot.as_mut() {
                agent.set_bash_approval_mode(next_mode);
                agent.set_full_access_mode(false);
            }
            let notice = match next_mode {
                BashApprovalMode::Always => "Bash approval set to always.",
                BashApprovalMode::Once => "Bash approval set to once.",
                BashApprovalMode::Suggestion => "Bash approval set to suggestion.",
            };
            mark_local_command(app, Some("updating approval mode".into()));
            app.push_notice(NoticeLevel::Info, notice);
        }
        LocalCommandKind::NowledgeMem => {
            handle_nowledge_mem_command(command.arg.as_deref(), app)?;
        }
        LocalCommandKind::Help => {
            mark_local_command(app, Some("opening help".into()));
            app.open_overlay(Overlay::Help(HelpTab::General));
        }
        LocalCommandKind::Clear => {
            mark_local_command(app, Some("clearing transcript".into()));
            app.reset_transcript();
        }
        LocalCommandKind::Compact => {
            request_maintenance(
                app,
                agent_slot,
                runtime_port,
                RuntimeMaintenanceCommand::Compact,
            )
            .await?;
        }
        LocalCommandKind::Context => {
            mark_local_command(app, Some("opening context".into()));
            app.open_overlay(Overlay::Context);
        }
        LocalCommandKind::Model => handle_model_command(command.arg.as_deref(), app)?,
        LocalCommandKind::Connect => handle_connect_command(app)?,
        LocalCommandKind::Mcp => handle_mcp_command(app),
        LocalCommandKind::Plan => {
            if app.is_busy() {
                app.push_notice(
                    NoticeLevel::Info,
                    "A task is already running. Wait for it to finish.",
                );
                return Ok(false);
            }
            mark_local_command(app, Some("entering planning mode".into()));
            app.clear_pending_plan_approval();
            app.permission_mode = PermissionMode::Custom;
            app.set_agent_execution_mode(AgentExecutionMode::Plan);
            if let Some(agent) = agent_slot.as_mut() {
                agent.set_execution_mode(AgentExecutionMode::Plan);
                agent.set_full_access_mode(false);
            }
            app.push_notice(
                NoticeLevel::Info,
                "Planning mode enabled. Read-only planning; approve to execute.",
            );
        }
        LocalCommandKind::Review => {
            request_maintenance(
                app,
                agent_slot,
                runtime_port,
                RuntimeMaintenanceCommand::Review,
            )
            .await?;
        }
        LocalCommandKind::Permissions => {
            let selected = app
                .pending_permission_mode
                .unwrap_or_else(|| app.effective_permission_mode());
            app.permission_picker_idx = crate::tui::permission_policy::PERMISSION_PRESETS
                .iter()
                .position(|preset| preset.mode == selected)
                .unwrap_or(0);
            mark_local_command(app, Some("opening permission picker".into()));
            app.open_overlay(Overlay::PermissionPicker);
        }
        LocalCommandKind::Quit => {
            mark_local_command(app, Some("quitting".into()));
            return Ok(true);
        }
        LocalCommandKind::Resume => {
            mark_local_command(app, Some("opening resume picker".into()));
            app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
        }
        LocalCommandKind::Status => {
            mark_local_command(app, Some("opening status".into()));
            app.open_overlay(Overlay::Status(StatusTab::Overview));
        }
        LocalCommandKind::Tasks => {
            handle_tasks_command(command.arg.as_deref(), app, agent_slot);
        }
        LocalCommandKind::Goal => {
            mark_local_command(app, Some("processing goal command".into()));
            super::goals::handle_command(command.arg.as_deref(), app, agent_slot, runtime_port)
                .await;
        }
        LocalCommandKind::Skills => {
            mark_local_command(app, Some("opening skills picker".into()));
            app.open_overlay(Overlay::SkillsPicker);
        }
    }
    if command_kind != LocalCommandKind::Tasks
        && let Some(agent) = agent_slot.as_ref()
    {
        app.apply_runtime_snapshot(
            agent,
            crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(agent, 0),
        );
    }
    Ok(false)
}

async fn request_maintenance(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
    command: RuntimeMaintenanceCommand,
) -> anyhow::Result<()> {
    if let Some(runtime_port) = runtime_port {
        runtime_port
            .send(RuntimeCommand::Maintenance(command))
            .await?;
    } else {
        match command {
            RuntimeMaintenanceCommand::Review => super::review::start(app, agent_slot),
            RuntimeMaintenanceCommand::Compact => {
                if let Some(agent) = agent_slot.take() {
                    start_compact_task(app, agent);
                } else {
                    app.push_notice(
                        NoticeLevel::Warning,
                        "No active agent available for compaction.",
                    );
                }
            }
            RuntimeMaintenanceCommand::Rebuild => {
                start_rebuild_task(app, agent_slot.as_ref().and_then(Agent::agent_tree_control))
            }
            RuntimeMaintenanceCommand::RefreshModelCatalog(_) => app.push_notice(
                NoticeLevel::Warning,
                "Model catalog loading requires a runtime client.",
            ),
        }
    }
    Ok(())
}

fn handle_connect_command(app: &mut TuiApp) -> anyhow::Result<()> {
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Provider));
    app.push_notice(
        NoticeLevel::Info,
        "Connect a provider — select the provider family, then configure API key and model.",
    );
    Ok(())
}

fn handle_nowledge_mem_command(arg: Option<&str>, app: &mut TuiApp) -> anyhow::Result<()> {
    if arg.is_some_and(|value| !value.trim().is_empty()) {
        app.push_notice(
            NoticeLevel::Warning,
            "/mem does not accept arguments. Choose a mode in the TUI.",
        );
    }
    app.open_overlay(Overlay::ListPicker(ListPickerKind::NowledgeMem));
    app.push_notice(NoticeLevel::Info, "Choose the builtin Nowledge Mem mode.");
    Ok(())
}

fn handle_model_command(arg: Option<&str>, app: &mut TuiApp) -> anyhow::Result<()> {
    if arg.is_some_and(|value| !value.trim().is_empty()) {
        app.push_notice(
            NoticeLevel::Warning,
            "/model does not accept arguments. Choose a model in the UI.",
        );
    }
    app.refresh_provider_connection_status();
    app.model_search_idx = app
        .available_unified_model_presets()
        .iter()
        .position(|preset| {
            preset.provider_id == app.config.provider
                && app.config.model.as_deref() == Some(&preset.model_id)
        })
        .unwrap_or(0);
    app.open_overlay(Overlay::ModelSearch);
    app.push_notice(
        NoticeLevel::Info,
        "Choose a model from an available provider. Run /connect to add or manage providers.",
    );
    Ok(())
}

fn handle_mcp_command(app: &mut TuiApp) {
    mark_local_command(app, Some("showing mcp status".into()));
    let project_root = command_project_root(app);
    match app
        .config_manager
        .load_mcp_registry_for_project(&project_root)
    {
        Ok(registry) => {
            let snapshot = McpStatusSnapshot::from_registry(&registry);
            publish_mcp_status_event(app, &snapshot);
            app.push_system(format_mcp_status(&snapshot), SystemMessageKind::MCPStatus);
            app.push_notice(NoticeLevel::Info, "MCP status updated.");
            if let Some(cache) = app.mcp_tool_cache.as_ref() {
                spawn_mcp_tool_cache_population(cache, &registry);
            }
        }
        Err(err) => {
            publish_mcp_status_load_failed_event(app, &format!("{err:#}"));
            app.push_system(
                format!("MCP Servers\n\nFailed to load MCP configuration:\n{err:#}"),
                SystemMessageKind::MCPStatus,
            );
            app.push_notice(NoticeLevel::Error, "MCP status failed.");
        }
    }
}

fn spawn_mcp_tool_cache_population(
    cache: &McpToolCache,
    registry: &McpRegistry,
) -> tokio::task::JoinHandle<()> {
    let servers: Vec<(String, std::sync::Arc<SourcedMcpServerConfig>)> = registry
        .servers
        .iter()
        .map(|(name, entry)| (name.clone(), std::sync::Arc::new(entry.clone())))
        .collect();
    let tools = cache.share();
    tokio::spawn(async move {
        {
            let Ok(mut map) = tools.lock() else {
                log::warn!("Cannot refresh MCP tools: tool cache lock is poisoned");
                return;
            };
            map.clear();
        }
        let tmp = McpToolCache::from_shared(tools);
        tmp.populate_from_registry_owned(servers).await;
    })
}

fn publish_mcp_status_load_failed_event(app: &TuiApp, message: &str) {
    if let Some(bus) = app.event_bus.as_ref()
        && bus.receiver_count() > 0
    {
        bus.send_with_provenance(
            AgentEvent::McpStatusLoadFailed {
                message: message.to_string(),
            },
            RuntimeProvenance::local_tui(app.snapshot.session_id.clone()),
        );
    }
}

fn publish_mcp_status_event(app: &TuiApp, snapshot: &McpStatusSnapshot) {
    if let Some(bus) = app.event_bus.as_ref()
        && bus.receiver_count() > 0
    {
        bus.send_with_provenance(
            AgentEvent::McpStatusUpdated(snapshot.clone()),
            RuntimeProvenance::local_tui(app.snapshot.session_id.clone()),
        );
    }
}

fn command_project_root(app: &TuiApp) -> PathBuf {
    let cwd = if app.snapshot.cwd.is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(&app.snapshot.cwd)
    };
    mcp_project_root_from_cwd(cwd)
}

fn handle_tasks_command(arg: Option<&str>, app: &mut TuiApp, agent_slot: &mut Option<Agent>) {
    mark_local_command(app, Some("processing shared task command".into()));
    let Some(requested) = arg.map(str::trim).filter(|value| !value.is_empty()) else {
        let tasks = &app.snapshot.shared_tasks;
        app.push_notice(
            NoticeLevel::Info,
            format!(
                "Active shared task list: {} ({} total, {} ready).",
                tasks.task_list_id, tasks.total, tasks.unblocked
            ),
        );
        return;
    };

    if let Some(agent) = agent_slot.as_mut() {
        agent.set_task_list_id(requested);
        app.apply_runtime_snapshot(
            agent,
            crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(agent, 0),
        );
    } else {
        app.switch_active_shared_task_list(requested);
    }
    let tasks = &app.snapshot.shared_tasks;
    app.push_notice(
        NoticeLevel::Info,
        format!(
            "Active shared task list: {} ({} total, {} ready).",
            tasks.task_list_id, tasks.total, tasks.unblocked
        ),
    );
}

fn mcp_project_root_from_cwd(cwd: PathBuf) -> PathBuf {
    for ancestor in cwd.ancestors() {
        if ancestor.join(".mcp.json").is_file() {
            return ancestor.to_path_buf();
        }
    }
    cwd
}

fn mark_local_command(app: &mut TuiApp, detail: Option<String>) {
    if !app.is_busy() {
        app.set_runtime_phase(RuntimePhase::LocalCommand, detail);
    }
}

#[cfg(test)]
#[path = "commands_test.rs"]
mod tests;

#[cfg(test)]
#[path = "goal_persistence_tests.rs"]
mod goal_persistence_tests;
