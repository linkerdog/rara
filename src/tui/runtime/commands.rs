use std::sync::Arc;

use super::super::state::{
    HelpTab, LocalCommand, LocalCommandKind, Overlay, PermissionMode, RuntimePhase, StatusTab,
    TuiApp,
};
use super::tasks::{start_compact_task, start_rebuild_task, start_review_task};
use crate::agent::{Agent, AgentExecutionMode, BashApprovalMode};
use crate::oauth::OAuthManager;

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
        LocalCommandKind::Model => "model",
        LocalCommandKind::Plan => "plan",
        LocalCommandKind::Quit => "quit",
        LocalCommandKind::Resume => "resume",
        LocalCommandKind::Review => "review",
        LocalCommandKind::Status => "status",
        LocalCommandKind::Skills => "skills",
        LocalCommandKind::Permissions => "permissions",
    });
    match command.kind {
        LocalCommandKind::Approval => {
            let next_mode = match app.bash_approval_mode {
                BashApprovalMode::Suggestion => BashApprovalMode::Always,
                BashApprovalMode::Once => BashApprovalMode::Suggestion,
                BashApprovalMode::Always => BashApprovalMode::Suggestion,
            };
            app.bash_approval_mode = next_mode;
            app.permission_mode = PermissionMode::Auto;
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
        LocalCommandKind::Plan => {
            app.set_runtime_phase(
                RuntimePhase::LocalCommand,
                Some("entering planning mode".into()),
            );
            app.set_pending_plan_approval(false);
            app.permission_mode = PermissionMode::Auto;
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
