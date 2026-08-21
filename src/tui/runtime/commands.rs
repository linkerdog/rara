use std::path::PathBuf;
use std::sync::Arc;

use super::super::state::{
    GoalStatus, HelpTab, ListPickerKind, LocalCommand, LocalCommandKind, Overlay, PermissionMode,
    RalphGoal, RuntimePhase, StatusTab, SystemMessageKind, TuiApp,
};
use super::tasks::{start_compact_task, start_rebuild_task, start_review_task};
use crate::agent::{Agent, AgentEvent, AgentExecutionMode, BashApprovalMode};
use crate::config::{McpRegistry, SourcedMcpServerConfig};
use crate::mcp_status::{McpStatusSnapshot, format_mcp_status};
use crate::mcp_tool_cache::McpToolCache;
use crate::oauth::OAuthManager;
use crate::runtime_control::RuntimeProvenance;
use crate::tui::runtime_port::{RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand};

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
        LocalCommandKind::Dream => "dream",
    });
    match command.kind {
        LocalCommandKind::Approval => {
            if app.is_busy() {
                app.push_notice("A task is already running. Wait for it to finish.");
                return Ok(false);
            }
            let next_mode = match app.bash_approval_mode {
                BashApprovalMode::Suggestion => BashApprovalMode::Always,
                BashApprovalMode::Once => BashApprovalMode::Suggestion,
                BashApprovalMode::Always => BashApprovalMode::Suggestion,
            };
            if next_mode == BashApprovalMode::Always {
                apply_permission_mode(app, agent_slot, PermissionMode::FullAccess);
                app.push_notice("Permission mode: full-access.");
                return Ok(false);
            }
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
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("updating approval mode".into()),
            );
            app.push_notice(notice);
        }
        LocalCommandKind::NowledgeMem => {
            handle_nowledge_mem_command(command.arg.as_deref(), app)?;
        }
        LocalCommandKind::Help => {
            app.set_runtime_phase(RuntimePhase::LocalCommand, Some("opening help".into()));
            app.open_overlay(Overlay::Help(HelpTab::General));
        }
        LocalCommandKind::Clear => {
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("clearing transcript".into()),
            );
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
            app.set_runtime_phase(RuntimePhase::LocalCommand, Some("opening context".into()));
            app.open_overlay(Overlay::Context);
        }
        LocalCommandKind::Model => handle_model_command(command.arg.as_deref(), app)?,
        LocalCommandKind::Connect => handle_connect_command(app)?,
        LocalCommandKind::Mcp => handle_mcp_command(app),
        LocalCommandKind::Plan => {
            if app.is_busy() {
                app.push_notice("A task is already running. Wait for it to finish.");
                return Ok(false);
            }
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("entering planning mode".into()),
            );
            app.clear_pending_plan_approval();
            app.permission_mode = PermissionMode::Custom;
            app.set_agent_execution_mode(AgentExecutionMode::Plan);
            if let Some(agent) = agent_slot.as_mut() {
                agent.set_execution_mode(AgentExecutionMode::Plan);
                agent.set_full_access_mode(false);
            }
            app.push_notice("Planning mode enabled. Read-only planning; approve to execute.");
        }
        LocalCommandKind::Review => {
            if app.is_busy() {
                app.push_notice("A task is already running. Wait for it to finish.");
            } else if let Some(agent) = agent_slot.take() {
                let diff = capture_git_diff(&app.snapshot.cwd);
                let prompt = if diff.is_empty() {
                    "No local git changes found. The working tree is clean.".to_string()
                } else {
                    let lines: Vec<&str> = diff.lines().collect();
                    if lines.len() > 800 {
                        let preview = lines
                            .iter()
                            .take(600)
                            .copied()
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!(
                            "Review the following code changes:\n\n```diff\n{preview}\n...\n```\n\n(Full diff truncated; use tools to inspect if needed.)"
                        )
                    } else {
                        format!("Review the following code changes:\n\n```diff\n{diff}\n```")
                    }
                };
                start_review_task(app, prompt, agent);
            }
        }
        LocalCommandKind::Permissions => {
            if app.is_busy() {
                app.push_notice("A task is already running. Wait for it to finish.");
                return Ok(false);
            }
            // Sync picker index to current mode before opening.
            let current_idx = match app.permission_mode {
                PermissionMode::Auto => 0,
                PermissionMode::AcceptEdits => 1,
                PermissionMode::ReadOnly => 2,
                PermissionMode::FullAccess => 3,
                PermissionMode::Custom => 0,
            };
            app.permission_picker_idx = current_idx;
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("opening permission picker".into()),
            );
            app.open_overlay(Overlay::PermissionPicker);
        }
        LocalCommandKind::Quit => {
            app.set_runtime_phase(RuntimePhase::LocalCommand, Some("quitting".into()));
            return Ok(true);
        }
        LocalCommandKind::Resume => {
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("opening resume picker".into()),
            );
            app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
        }
        LocalCommandKind::Status => {
            app.set_runtime_phase(RuntimePhase::LocalCommand, Some("opening status".into()));
            app.open_overlay(Overlay::Status(StatusTab::Overview));
        }
        LocalCommandKind::Tasks => {
            handle_tasks_command(command.arg.as_deref(), app, agent_slot);
        }
        LocalCommandKind::Dream => {
            if let Some(agent) = agent_slot.as_mut() {
                let summary = agent.consolidation_scheduler.status();
                app.set_runtime_phase(RuntimePhase::LocalCommand, Some(summary));
            } else {
                app.push_notice("Memory consolidation is not available until an agent is ready.");
            }
        }

        LocalCommandKind::Goal => {
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("processing goal command".into()),
            );
            let arg = command.arg.as_deref().unwrap_or("").trim();
            match arg {
                "" => {
                    // /goal with no subcommand: show current goal status
                    if let Some(goal) = app.goal.as_ref() {
                        let status_str = match goal.status {
                            GoalStatus::Pursuing => "active",
                            GoalStatus::Paused => "paused",
                            GoalStatus::Complete => "complete",
                            GoalStatus::BudgetLimited => "budget-limited",
                        };
                        let usage = goal
                            .token_budget
                            .map(|budget| {
                                format!(" · {} / {budget} tokens", goal.tokens_used.min(budget))
                            })
                            .unwrap_or_default();
                        let notice =
                            format!("Goal: {} [{status_str}]{usage}", goal.objective.as_str());
                        app.push_notice(notice);
                    } else {
                        app.push_notice("No active goal. Use /help for /goal details.");
                    }
                }
                "pause" => {
                    if let Some(goal) = app.goal.as_mut() {
                        if goal.status == GoalStatus::Pursuing {
                            goal.status = GoalStatus::Paused;
                            app.push_notice("Goal paused. Use /goal resume to continue.");
                            *app.goal_handle.write().unwrap() = app.goal.clone();
                        } else {
                            app.push_notice("Goal is not currently pursuing; nothing to pause.");
                        }
                    } else {
                        app.push_notice("No active goal to pause.");
                    }
                }
                "resume" => {
                    if let Some(goal) = app.goal.as_mut() {
                        if goal.status == GoalStatus::Paused {
                            goal.status = GoalStatus::Pursuing;
                            *app.goal_handle.write().unwrap() = app.goal.clone();
                            app.push_notice("Goal resumed. The agent will continue working.");
                        } else {
                            app.push_notice("Goal is not paused; nothing to resume.");
                        }
                    } else {
                        app.push_notice("No active goal to resume.");
                    }
                }
                "clear" => {
                    if app.goal.is_some() {
                        app.goal = None;
                        *app.goal_handle.write().unwrap() = None;
                        app.push_notice("Goal cleared.");
                    } else {
                        app.push_notice("No active goal to clear.");
                    }
                }
                objective => {
                    if app.goal.is_some() {
                        app.push_notice(
                            "A goal already exists. Use /goal clear before setting a new goal.",
                        );
                        return Ok(false);
                    }
                    match parse_goal_objective_and_budget(objective) {
                        Ok((objective_clean, budget)) => {
                            app.goal = Some(RalphGoal::new(objective_clean.clone(), budget));
                            *app.goal_handle.write().unwrap() = app.goal.clone();
                            if let Some(db) = app.state_db.as_ref()
                                && let Some(ref goal) = app.goal
                            {
                                match serde_json::to_value(goal) {
                                    Ok(v) => {
                                        if let Err(e) = db.save_goal(&app.snapshot.session_id, &v) {
                                            app.push_notice(format!(
                                                "Goal saved but persistence failed: {e}"
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        app.push_notice(format!(
                                            "Goal saved but serialisation failed: {e}"
                                        ));
                                    }
                                }
                            }
                            let mut notice = format!("Goal set: {}", objective_clean);
                            if let Some(b) = budget {
                                notice.push_str(&format!(" [budget: {b} tokens]"));
                            }
                            app.push_notice(notice);
                        }
                        Err(message) => app.push_notice(message),
                    }
                }
            }
        }
        LocalCommandKind::Skills => {
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("opening skills picker".into()),
            );
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
            RuntimeMaintenanceCommand::Compact => {
                if let Some(agent) = agent_slot.take() {
                    start_compact_task(app, agent);
                } else {
                    app.push_notice("No active agent available for compaction.");
                }
            }
            RuntimeMaintenanceCommand::Rebuild => {
                start_rebuild_task(app, agent_slot.as_ref().and_then(Agent::agent_tree_control))
            }
            RuntimeMaintenanceCommand::RefreshModelCatalog(_) => {
                app.push_notice("Model catalog loading requires a runtime client.")
            }
        }
    }
    Ok(())
}

fn handle_connect_command(app: &mut TuiApp) -> anyhow::Result<()> {
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Provider));
    app.bottom_pane.notice = Some(
        "Connect a provider — select the provider family, then configure API key and model.".into(),
    );
    Ok(())
}

fn handle_nowledge_mem_command(arg: Option<&str>, app: &mut TuiApp) -> anyhow::Result<()> {
    if arg.is_some_and(|value| !value.trim().is_empty()) {
        app.push_notice("/mem does not accept arguments. Choose a mode in the TUI.");
    }
    app.open_overlay(Overlay::ListPicker(ListPickerKind::NowledgeMem));
    app.bottom_pane.notice = Some("Choose the builtin Nowledge Mem mode.".into());
    Ok(())
}

fn parse_goal_objective_and_budget(input: &str) -> Result<(String, Option<u32>), String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Goal objective cannot be empty.".into());
    }

    if let Some(rest) = input.strip_prefix("--tokens ") {
        let (budget_raw, objective) = rest
            .trim()
            .split_once(char::is_whitespace)
            .ok_or_else(|| "Usage: /goal --tokens <N> <objective>.".to_string())?;
        let budget = parse_goal_token_budget(budget_raw)
            .ok_or_else(|| format!("Invalid goal token budget: {budget_raw}."))?;
        let objective = objective.trim();
        if objective.is_empty() {
            return Err("Goal objective cannot be empty.".into());
        }
        return Ok((objective.to_string(), Some(budget)));
    }

    if let Some((first, rest)) = input.split_once(char::is_whitespace)
        && first.bytes().all(|b| b.is_ascii_digit())
    {
        let budget = parse_goal_token_budget(first)
            .ok_or_else(|| format!("Invalid goal token budget: {first}."))?;
        let objective = rest.trim();
        if objective.is_empty() {
            return Err("Goal objective cannot be empty.".into());
        }
        return Ok((objective.to_string(), Some(budget)));
    }

    Ok((input.to_string(), None))
}

fn parse_goal_token_budget(input: &str) -> Option<u32> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (number, multiplier) = match trimmed.as_bytes().last().copied() {
        Some(b'k') | Some(b'K') => (&trimmed[..trimmed.len() - 1], 1_000.0),
        Some(b'm') | Some(b'M') => (&trimmed[..trimmed.len() - 1], 1_000_000.0),
        _ => (trimmed, 1.0),
    };
    if number.is_empty() || number.starts_with('-') {
        return None;
    }

    let value = number.parse::<f64>().ok()? * multiplier;
    if !value.is_finite() || value <= 0.0 || value > u32::MAX as f64 {
        return None;
    }

    Some(value.round() as u32)
}

fn handle_model_command(arg: Option<&str>, app: &mut TuiApp) -> anyhow::Result<()> {
    if arg.is_some_and(|value| !value.trim().is_empty()) {
        app.push_notice("/model does not accept arguments. Choose a model in the UI.");
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
    app.bottom_pane.notice = Some(
        "Choose a model from an available provider. Run /connect to add or manage providers."
            .into(),
    );
    Ok(())
}

fn handle_mcp_command(app: &mut TuiApp) {
    app.set_runtime_phase(
        RuntimePhase::LocalCommand,
        Some("showing mcp status".into()),
    );
    let project_root = command_project_root(app);
    match app
        .config_manager
        .load_mcp_registry_for_project(&project_root)
    {
        Ok(registry) => {
            let snapshot = McpStatusSnapshot::from_registry(&registry);
            publish_mcp_status_event(app, &snapshot);
            app.push_system(format_mcp_status(&snapshot), SystemMessageKind::MCPStatus);
            app.bottom_pane.notice = Some("MCP status updated.".into());
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
            app.bottom_pane.notice = Some("MCP status failed.".into());
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
            let mut map = tools.lock().unwrap();
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
    app.set_runtime_phase(
        RuntimePhase::LocalCommand,
        Some("processing shared task command".into()),
    );
    let Some(requested) = arg.map(str::trim).filter(|value| !value.is_empty()) else {
        let tasks = &app.snapshot.shared_tasks;
        app.push_notice(format!(
            "Active shared task list: {} ({} total, {} ready).",
            tasks.task_list_id, tasks.total, tasks.unblocked
        ));
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
    app.push_notice(format!(
        "Active shared task list: {} ({} total, {} ready).",
        tasks.task_list_id, tasks.total, tasks.unblocked
    ));
}

fn mcp_project_root_from_cwd(cwd: PathBuf) -> PathBuf {
    for ancestor in cwd.ancestors() {
        if ancestor.join(".mcp.json").is_file() {
            return ancestor.to_path_buf();
        }
    }
    cwd
}

fn capture_git_diff(cwd: &str) -> String {
    use std::path::Path;
    use std::process::Command;
    let dir = if cwd.is_empty() {
        None
    } else {
        Some(Path::new(cwd))
    };
    let cmd = |args: &[&str]| {
        let mut c = Command::new("git");
        c.args(args);
        if let Some(d) = dir {
            c.current_dir(d);
        }
        c.output()
    };
    let run = |args| -> Option<String> {
        cmd(args)
            .ok()
            .and_then(|out| {
                if !out.stderr.is_empty() {
                    let _stderr_msg = String::from_utf8_lossy(&out.stderr);
                }
                String::from_utf8(out.stdout).ok()
            })
            .filter(|s| !s.trim().is_empty())
    };
    let staged = run(&["diff", "--staged"]);
    let unstaged = run(&["diff"]);
    match (staged, unstaged) {
        (Some(s), Some(u)) => format!("{s}\n{u}"),
        (Some(s), None) => s,
        (None, Some(u)) => u,
        (None, None) => String::new(),
    }
}

pub(crate) fn apply_permission_mode(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    mode: PermissionMode,
) {
    use std::sync::atomic::Ordering;

    let (execution, approval, allow_net, full_access) = match mode {
        PermissionMode::Auto => (
            AgentExecutionMode::Execute,
            BashApprovalMode::Always,
            false,
            false,
        ),
        PermissionMode::AcceptEdits => (
            AgentExecutionMode::Execute,
            BashApprovalMode::Suggestion,
            false,
            false,
        ),
        PermissionMode::ReadOnly => (
            AgentExecutionMode::Plan,
            BashApprovalMode::Suggestion,
            false,
            false,
        ),
        PermissionMode::FullAccess => (
            AgentExecutionMode::Execute,
            BashApprovalMode::Always,
            true,
            true,
        ),
        PermissionMode::Custom => return,
    };

    app.permission_mode = mode;
    app.set_agent_execution_mode(execution);
    app.bash_approval_mode = approval;
    app.sandbox_network_access
        .store(allow_net, Ordering::Relaxed);

    if let Some(agent) = agent_slot.as_mut() {
        agent.set_execution_mode(execution);
        agent.set_bash_approval_mode(approval);
        agent.set_full_access_mode(full_access);
    }

    app.set_runtime_phase(
        RuntimePhase::LocalCommand,
        Some("updating permissions".into()),
    );
    app.clear_pending_plan_approval();
}

#[cfg(test)]
#[path = "commands_test.rs"]
mod tests;
