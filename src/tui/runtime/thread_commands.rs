use std::time::Instant;

use anyhow::{Context, Result, ensure};
use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::runtime_client::{RuntimeClient, ThreadCommandOutcome, ThreadCommandResult};
use crate::tui::state::{NoticeLevel, RunningTask, RuntimePhase, TaskCompletion, TaskKind, TuiApp};

#[cfg(test)]
mod tests;

pub(super) fn start_rename(app: &mut TuiApp, runtime: &RuntimeClient, title: String) {
    let result = (|| {
        ensure!(
            !app.is_busy(),
            "wait for the active task before renaming a thread"
        );
        let db = app
            .state_db
            .clone()
            .context("session storage is unavailable")?;
        app.persist_runtime_state();
        let storage = app
            .storage
            .as_ref()
            .context("session storage worker is unavailable")?;
        runtime.rename_thread(storage, db, title)
    })();
    wait_for_command(app, result, "renaming thread");
}

pub(super) fn start_new(app: &mut TuiApp, runtime: &RuntimeClient) {
    let result = (|| {
        ensure!(
            !app.is_busy(),
            "wait for the active task before starting a new thread"
        );
        ensure!(
            app.bottom_pane.queued_follow_up_messages.is_empty()
                && app.bottom_pane.pending_follow_up_messages.is_empty(),
            "send or remove queued prompts before starting a new thread"
        );
        let db = app
            .state_db
            .clone()
            .context("session storage is unavailable")?;
        app.finalize_active_turn();
        app.persist_runtime_state();
        let storage = app
            .storage
            .as_ref()
            .context("session storage worker is unavailable")?;
        runtime.create_thread(storage, db)
    })();
    wait_for_command(app, result, "starting a new thread");
}

fn wait_for_command(
    app: &mut TuiApp,
    result: Result<tokio::sync::oneshot::Receiver<Result<ThreadCommandResult>>>,
    description: &str,
) {
    let receipt = match result {
        Ok(receipt) => receipt,
        Err(error) => {
            app.push_notice(
                NoticeLevel::Error,
                format!("Thread command failed: {error:#}"),
            );
            return;
        }
    };
    let (sender, receiver) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let _sender = sender;
        TaskCompletion::ThreadCommand {
            result: receipt
                .await
                .context("storage worker stopped during thread command")
                .and_then(|result| result),
        }
    });
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::ThreadCommand,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: None,
    });
    app.set_runtime_phase(RuntimePhase::LocalCommand, Some(description.into()));
    app.push_notice(NoticeLevel::Info, format!("{}...", description));
}

pub(super) fn start_export(app: &mut TuiApp, runtime: &RuntimeClient, path: Option<String>) {
    let result = (|| {
        ensure!(
            !app.is_busy(),
            "wait for the active task before exporting a thread"
        );
        let db = app
            .state_db
            .clone()
            .context("session storage is unavailable")?;
        app.finalize_active_turn();
        app.persist_runtime_state();
        let storage = app
            .storage
            .as_ref()
            .context("session storage worker is unavailable")?;
        runtime.export_thread(storage, db, path)
    })();
    wait_for_command(app, result, "exporting thread");
}

pub(super) fn finish(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    result: Result<ThreadCommandResult>,
) {
    let result = result.and_then(|saved| {
        let agent = agent_slot.as_mut().context("runtime agent is not ready")?;
        ensure!(
            agent.session_id == saved.source_session_id,
            "the command completed for a previous thread"
        );
        match saved.outcome {
            ThreadCommandOutcome::Exported { path } => {
                app.push_notice(
                    NoticeLevel::Info,
                    format!("Exported conversation to {}", path.display()),
                );
            }
            ThreadCommandOutcome::Renamed { title } => {
                app.push_notice(NoticeLevel::Info, format!("Thread renamed to: {title}"));
                app.set_terminal_thread_title(Some(title));
            }
            ThreadCommandOutcome::Created { session_id, goal } => {
                // Runtime services are unchanged; reuse their projection rather
                // than rediscovering workspace extensions during completion.
                let extensions = crate::tui::state::RuntimeExtensionSnapshot {
                    skill_count: app.snapshot.extension_skill_count,
                    skill_scopes: app.snapshot.extension_skill_scopes.clone(),
                    hook_count: app.snapshot.extension_hook_count,
                    command_count: app.snapshot.extension_command_count,
                    agent_count: app.snapshot.extension_agent_count,
                    agent_status_lines: app.snapshot.extension_agent_status_lines.clone(),
                };
                RuntimeClient::apply_new_thread(agent, &app.goal_handle, session_id, goal);
                // The old conversation was committed before preparation. Avoid
                // reset_transcript(), which enqueues writes against the old snapshot.
                app.restore_committed_turns(Vec::new());
                app.snapshot.pending_interactions.clear();
                app.snapshot.completed_interactions.clear();
                app.snapshot.subagents.clear();
                app.bottom_pane.pending_planning_suggestion = None.into();
                app.running_tool_boundary_count = 0;
                app.pending_goal_resume = None;
                app.goal_ui = Default::default();
                app.goal = None;
                app.approval_picker_idx = 0;
                app.apply_runtime_snapshot(agent, extensions);
                app.terminal_feedback = Default::default();
                app.push_notice(
                    NoticeLevel::Info,
                    format!("Started thread {}.", agent.session_id),
                );
            }
        }
        app.set_runtime_phase(RuntimePhase::Idle, None);
        Ok(())
    });
    if let Err(error) = result {
        log::warn!("Thread command failed: {error:#}");
        app.push_notice(
            NoticeLevel::Error,
            format!("Thread command failed: {error:#}"),
        );
        app.set_runtime_phase(RuntimePhase::Failed, Some("thread command failed".into()));
    }
    if let Some(mode) = app.pending_permission_mode.take() {
        super::permissions::request_permission_mode(app, agent_slot, mode);
    }
}
