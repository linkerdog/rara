use std::sync::Arc;

use super::command::parse_local_command;
use super::input_control;
use super::runtime::execute_local_command;
use super::runtime_port::{RuntimeClientPort, RuntimeCommand};
use super::state::{
    ActivePendingInteractionKind, LocalCommandKind, OpenAiModelPickerAction, TuiApp,
};
use crate::agent::Agent;

mod pending;

pub(crate) async fn handle_submit(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<crate::oauth::OAuthManager>,
) -> anyhow::Result<bool> {
    handle_submit_inner(app, agent_slot, oauth_manager, None).await
}

pub(crate) async fn handle_submit_with_port(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<crate::oauth::OAuthManager>,
    runtime_port: &dyn RuntimeClientPort,
) -> anyhow::Result<bool> {
    handle_submit_inner(app, agent_slot, oauth_manager, Some(runtime_port)).await
}

async fn handle_submit_inner(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<crate::oauth::OAuthManager>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<bool> {
    if app.bottom_pane.input.is_empty() {
        if let Some(interaction) = app.active_pending_interaction()
            && matches!(
                interaction.kind,
                ActivePendingInteractionKind::ShellApproval
            )
        {
            app.push_notice(
                "Approval pending. Use Up/Down and Enter, or press 1-4 to choose an option.",
            );
            return Ok(false);
        }
        // Lightweight feedback so the user knows Enter was received.
        // Don't overwrite existing notices (e.g., status-info after a command).
        if app.bottom_pane.notice.is_none() {
            app.bottom_pane.notice = Some("Ready.".into());
        }
        return Ok(false);
    }
    let input = std::mem::take(&mut app.bottom_pane.input);
    app.bottom_pane.input_cursor_offset = None;
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        // Whitespace-only input: same lightweight feedback.
        if app.bottom_pane.notice.is_none() {
            app.bottom_pane.notice = Some("Ready.".into());
        }
        app.bottom_pane.input.clear();
        return Ok(false);
    }
    app.record_input_history(&trimmed);

    if app.is_busy() {
        if trimmed.starts_with('/') {
            if let Some(command) = parse_local_command(&trimmed)
                && matches!(command.kind, LocalCommandKind::Quit)
            {
                save_before_quit(app);
                return execute_local_command(command, app, agent_slot, oauth_manager).await;
            }
            app.push_notice(
                "A task is already running. Wait for it to finish before running a slash command.",
            );
        } else if let Some(runtime_port) = runtime_port {
            runtime_port
                .send(RuntimeCommand::Input(
                    crate::runtime_control::InputControlRequest::SubmitUserPrompt {
                        prompt: trimmed,
                    },
                ))
                .await?;
        } else {
            input_control::submit_user_prompt(app, agent_slot, trimmed);
        }
        return Ok(false);
    }

    if app.has_pending_plan_approval() && !trimmed.starts_with('/') {
        if pending::handle_pending_option_submit(app, agent_slot, &trimmed, runtime_port).await? {
            return Ok(false);
        }
        handle_pending_plan_approval_submit(app);
        return Ok(false);
    }
    if let Some(command) = parse_local_command(&trimmed) {
        let should_quit = execute_local_command(command, app, agent_slot, oauth_manager).await?;
        if should_quit {
            save_before_quit(app);
            return Ok(true);
        }
    } else if trimmed.starts_with('/') {
        app.push_notice(format!("Unknown command '{}'. Use /help.", trimmed));
    } else if pending::handle_pending_option_submit(app, agent_slot, &trimmed, runtime_port).await?
    {
        return Ok(false);
    } else if let Some(runtime_port) = runtime_port {
        runtime_port
            .send(RuntimeCommand::Input(
                crate::runtime_control::InputControlRequest::SubmitUserPrompt { prompt: trimmed },
            ))
            .await?;
    } else {
        input_control::submit_user_prompt(app, agent_slot, trimmed);
    }
    Ok(false)
}

/// Persist the active turn and runtime state before quitting.
fn save_before_quit(app: &mut TuiApp) {
    // Dismiss all stacked overlays before quitting, so the quit phase
    // transition is the last thing the user sees.
    while app.overlay.is_some() {
        app.dismiss_overlay();
    }
    app.finalize_active_turn();
    app.persist_runtime_state();
}

pub(crate) fn apply_openai_model_picker_action(
    app: &mut TuiApp,
    action: OpenAiModelPickerAction,
) -> anyhow::Result<()> {
    match action {
        OpenAiModelPickerAction::SelectProfile => {
            if let Some(label) = app.select_openai_model_picker_profile() {
                app.config_manager.save(&app.config)?;
                if app.openai_profile_needs_setup() {
                    app.bottom_pane.notice = Some(format!("Selected endpoint profile: {label}"));
                    app.begin_active_openai_profile_setup();
                } else {
                    super::runtime::start_rebuild_task(app);
                }
            }
        }
        OpenAiModelPickerAction::DeleteProfile => {
            if let Some(label) = app.delete_active_openai_profile() {
                app.config_manager.save(&app.config)?;
                if app.openai_profile_needs_setup() {
                    app.bottom_pane.notice = Some(format!("Deleted endpoint profile: {label}"));
                    app.begin_active_openai_profile_setup();
                } else {
                    app.bottom_pane.notice = Some(format!("Deleted endpoint profile: {label}"));
                    super::runtime::start_rebuild_task(app);
                }
            } else {
                app.push_notice("Cannot delete the only endpoint profile.");
            }
        }
    }
    Ok(())
}

fn handle_pending_plan_approval_submit(app: &mut TuiApp) {
    app.push_notice(
        "A plan is waiting for review. Use 1 approve, 2 keep planning <feedback>, or 3 reject <feedback>.",
    );
}

pub(crate) fn clamp_command_palette_selection(app: &mut TuiApp) {
    let len = super::command::palette_commands(app, app.command_query()).len();
    if len == 0 {
        app.command_palette_idx = 0;
    } else if app.command_palette_idx >= len {
        app.command_palette_idx = len - 1;
    }
}
