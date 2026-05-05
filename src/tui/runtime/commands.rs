use std::path::PathBuf;
use std::sync::Arc;

use super::super::state::{
    GoalStatus, HelpTab, LocalCommand, LocalCommandKind, Overlay, PermissionMode, RalphGoal,
    RuntimePhase, StatusTab, TuiApp,
};
use super::tasks::{start_compact_task, start_rebuild_task, start_review_task};
use crate::agent::{Agent, AgentEvent, AgentExecutionMode, BashApprovalMode};
use crate::mcp_status::{McpStatusSnapshot, format_mcp_status};
use crate::oauth::OAuthManager;
use crate::runtime_control::RuntimeProvenance;

pub(super) async fn execute_local_command(
    command: LocalCommand,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
) -> anyhow::Result<bool> {
    app.remember_command(match command.kind {
        LocalCommandKind::Approval => "approval",
        LocalCommandKind::BaseUrl => "base-url",
        LocalCommandKind::Clear => "clear",
        LocalCommandKind::Compact => "compact",
        LocalCommandKind::Context => "context",
        LocalCommandKind::Help => "help",
        LocalCommandKind::Login => "login",
        LocalCommandKind::Logout => "logout",
        LocalCommandKind::Mcp => "mcp",
        LocalCommandKind::Model => "model",
        LocalCommandKind::Plan => "plan",
        LocalCommandKind::Quit => "quit",
        LocalCommandKind::Resume => "resume",
        LocalCommandKind::Review => "review",
        LocalCommandKind::Status => "status",
        LocalCommandKind::Skills => "skills",
        LocalCommandKind::Permissions => "permissions",
        LocalCommandKind::Goal => "goal",
    });
    match command.kind {
        LocalCommandKind::Approval => {
            let next_mode = match app.bash_approval_mode {
                BashApprovalMode::Suggestion => BashApprovalMode::Always,
                BashApprovalMode::Once => BashApprovalMode::Suggestion,
                BashApprovalMode::Always => BashApprovalMode::Suggestion,
            };
            app.bash_approval_mode = next_mode;
            app.permission_mode = PermissionMode::Custom;
            if let Some(agent) = agent_slot.as_mut() {
                agent.set_bash_approval_mode(next_mode);
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
        LocalCommandKind::BaseUrl => handle_base_url_command(command.arg.as_deref(), app)?,
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
            if let Some(agent) = agent_slot.take() {
                start_compact_task(app, agent);
            } else {
                app.push_notice("No active agent available for compaction.");
            }
        }
        LocalCommandKind::Context => {
            app.set_runtime_phase(RuntimePhase::LocalCommand, Some("opening context".into()));
            app.open_overlay(Overlay::Context);
        }
        LocalCommandKind::Login => {
            if app.is_busy() {
                app.push_notice("A task is already running. Wait for it to finish.");
            } else {
                app.open_overlay(Overlay::AuthModePicker);
            }
        }
        LocalCommandKind::Logout => {
            if app.is_busy() {
                app.push_notice("A task is already running. Wait for it to finish.");
            } else {
                let removed = oauth_manager.clear_saved_auth()?;
                app.config.clear_provider_api_key("codex");
                app.config_manager.save(&app.config)?;
                app.push_notice(if removed {
                    "Cleared the saved provider credential.".to_string()
                } else {
                    "No saved provider credential was present.".to_string()
                });
                if app.config.provider == "codex" {
                    start_rebuild_task(app);
                }
            }
        }
        LocalCommandKind::Model => handle_model_command(command.arg.as_deref(), app)?,
        LocalCommandKind::Mcp => handle_mcp_command(app),
        LocalCommandKind::Plan => {
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("entering planning mode".into()),
            );
            app.set_pending_plan_approval(false);
            app.permission_mode = PermissionMode::Custom;
            app.set_agent_execution_mode(AgentExecutionMode::Plan);
            if let Some(agent) = agent_slot.as_mut() {
                agent.set_execution_mode(AgentExecutionMode::Plan);
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
            let next_mode = app.permission_mode.cycle();
            app.permission_mode = next_mode;
            apply_permission_mode(app, agent_slot, next_mode);
            let notice = format!(
                "Permission mode: {label}.",
                label = app.permission_mode_label()
            );
            app.push_notice(notice);
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
            app.open_overlay(Overlay::ResumePicker);
        }
        LocalCommandKind::Status => {
            app.set_runtime_phase(RuntimePhase::LocalCommand, Some("opening status".into()));
            app.open_overlay(Overlay::Status(StatusTab::Overview));
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
                            GoalStatus::Pursuing => "pursuing",
                            GoalStatus::Paused => "paused",
                            GoalStatus::Achieved => "achieved",
                            GoalStatus::Unmet => "unmet",
                            GoalStatus::BudgetLimited => "budget-limited",
                        };
                        let mut notice = format!(
                            "Goal: {} [{}] · turns={} · tokens={}",
                            goal.objective, status_str, goal.turns_completed, goal.tokens_used
                        );
                        if let Some(budget) = goal.token_budget {
                            notice.push_str(&format!("/{budget}"));
                        }
                        app.push_notice(notice);
                    } else {
                        app.push_notice("No active goal. Set one with /goal <objective>.");
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
                    // /goal <objective> — start a new goal
                    let mut budget: Option<u32> = None;
                    let objective_clean = if let Some((budget_str, obj)) = objective.split_once(' ')
                    {
                        if let Ok(b) = budget_str.trim().parse::<u32>() {
                            budget = Some(b);
                            obj
                        } else {
                            objective
                        }
                    } else {
                        objective
                    };
                    if objective_clean.is_empty() {
                        app.push_notice("Goal objective cannot be empty.");
                    } else {
                        app.goal = Some(RalphGoal {
                            objective: objective_clean.to_string(),
                            status: GoalStatus::Pursuing,
                            token_budget: budget,
                            tokens_used: 0,
                            turns_completed: 0,
                        });
                        *app.goal_handle.write().unwrap() = app.goal.clone();
                        let mut notice = format!("Goal set: {}", objective_clean);
                        if let Some(b) = budget {
                            notice.push_str(&format!(" [budget: {b} tokens]"));
                        }
                        app.push_notice(notice);
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
    if let Some(agent) = agent_slot.as_ref() {
        app.sync_snapshot(agent);
    }
    Ok(false)
}

fn handle_model_command(arg: Option<&str>, app: &mut TuiApp) -> anyhow::Result<()> {
    if arg.map(str::trim).filter(|arg| !arg.is_empty()).is_some() {
        app.push_notice("/model does not accept arguments. Use the interactive menu.");
    }
    app.open_overlay(Overlay::ProviderPicker);
    app.notice = Some("Opened provider picker.".into());
    Ok(())
}

fn handle_base_url_command(arg: Option<&str>, app: &mut TuiApp) -> anyhow::Result<()> {
    if arg.map(str::trim).filter(|arg| !arg.is_empty()).is_some() {
        app.push_notice("/base-url does not accept arguments. Edit the value in the TUI.");
    }
    app.open_overlay(Overlay::BaseUrlEditor);
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
            app.push_entry("System", format_mcp_status(&snapshot));
            app.notice = Some("MCP status updated.".into());
        }
        Err(err) => {
            publish_mcp_status_load_failed_event(app, &format!("{err:#}"));
            app.push_entry(
                "System",
                format!("MCP Servers\n\nFailed to load MCP configuration:\n{err:#}"),
            );
            app.notice = Some("MCP status failed.".into());
        }
    }
}

fn publish_mcp_status_load_failed_event(app: &TuiApp, message: &str) {
    if let Some(bus) = app.event_bus.as_ref() {
        if bus.receiver_count() > 0 {
            bus.send_with_provenance(
                AgentEvent::McpStatusLoadFailed {
                    message: message.to_string(),
                },
                RuntimeProvenance::local_tui(app.snapshot.session_id.clone()),
            );
        }
    }
}

fn publish_mcp_status_event(app: &TuiApp, snapshot: &McpStatusSnapshot) {
    if let Some(bus) = app.event_bus.as_ref() {
        if bus.receiver_count() > 0 {
            bus.send_with_provenance(
                AgentEvent::McpStatusUpdated(snapshot.clone()),
                RuntimeProvenance::local_tui(app.snapshot.session_id.clone()),
            );
        }
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

fn apply_permission_mode(app: &mut TuiApp, agent_slot: &mut Option<Agent>, mode: PermissionMode) {
    use std::sync::atomic::Ordering;

    let (execution, approval, allow_net) = match mode {
        PermissionMode::Auto => (AgentExecutionMode::Execute, BashApprovalMode::Always, false),
        PermissionMode::AcceptEdits => (
            AgentExecutionMode::Execute,
            BashApprovalMode::Suggestion,
            false,
        ),
        PermissionMode::ReadOnly => (
            AgentExecutionMode::Plan,
            BashApprovalMode::Suggestion,
            false,
        ),
        PermissionMode::FullAccess => (AgentExecutionMode::Execute, BashApprovalMode::Always, true),
        PermissionMode::Custom => return,
    };

    app.set_agent_execution_mode(execution);
    app.bash_approval_mode = approval;
    app.sandbox_network_access
        .store(allow_net, Ordering::Relaxed);

    if let Some(agent) = agent_slot.as_mut() {
        agent.set_execution_mode(execution);
        agent.set_bash_approval_mode(approval);
    }

    app.set_runtime_phase(
        RuntimePhase::LocalCommand,
        Some("updating permissions".into()),
    );
    app.set_pending_plan_approval(false);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use super::{handle_mcp_command, mcp_project_root_from_cwd};
    use crate::agent::AgentEvent;
    use crate::config::ConfigManager;
    use crate::runtime_event_bus::RuntimeEventBus;
    use crate::tui::state::TuiApp;

    #[test]
    fn mcp_project_root_walks_up_to_project_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let nested = project.join("src").join("bin");
        fs::create_dir_all(&nested).expect("nested dirs");
        fs::write(project.join(".mcp.json"), r#"{"mcpServers":{}}"#).expect("project config");

        assert_eq!(mcp_project_root_from_cwd(nested), project);
    }

    #[test]
    fn mcp_project_root_keeps_cwd_when_no_project_config_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("project").join("src");
        fs::create_dir_all(&cwd).expect("cwd");

        assert_eq!(mcp_project_root_from_cwd(cwd.clone()), cwd);
    }

    #[test]
    fn mcp_command_publishes_structured_status_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("config.toml"),
            r#"
[mcp_servers.docs]
command = "docs-server"
"#,
        )
        .expect("user config");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).expect("project");

        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .expect("app");
        app.snapshot.cwd = project.to_string_lossy().to_string();
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut receiver = bus.subscribe();
        app.event_bus = Some(bus);

        handle_mcp_command(&mut app);

        let event = receiver.try_recv().expect("mcp status event");
        let AgentEvent::McpStatusUpdated(snapshot) = event else {
            panic!("expected mcp status event");
        };
        assert_eq!(snapshot.servers.len(), 1);
        assert_eq!(snapshot.servers[0].name, "docs");
    }

    #[test]
    fn mcp_command_publishes_structured_load_failure_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("config.toml"), "[mcp_servers.docs\n").expect("user config");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).expect("project");

        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .expect("app");
        app.snapshot.cwd = project.to_string_lossy().to_string();
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut receiver = bus.subscribe();
        app.event_bus = Some(bus);

        handle_mcp_command(&mut app);

        let event = receiver.try_recv().expect("mcp failure event");
        let AgentEvent::McpStatusLoadFailed { message } = event else {
            panic!("expected mcp load failure event");
        };
        assert!(message.contains("config.toml"));
    }
}
