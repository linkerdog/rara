use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Instant;

use crate::agent::Agent;
use crate::tui::state::NoticeLevel;
use crate::tui::state::{RunningTask, RuntimePhase, TaskCompletion, TaskKind, TuiApp};

mod git_diff;

#[derive(Debug, Default)]
struct CapturedDiff {
    text: String,
    truncated: bool,
}

/// Captures both Git diff scopes without blocking the UI or retaining unbounded
/// output. Dropping the future must terminate and reap any owned child process.
#[async_trait::async_trait]
trait DiffCapture: Send + Sync {
    async fn capture(&self, cwd: &Path) -> anyhow::Result<CapturedDiff>;
}

pub(crate) enum ReviewPreparation {
    Clean,
    Prompt(String),
    Cancelled,
}

pub(in crate::tui) fn start(app: &mut TuiApp, agent: &Option<Agent>) {
    if app.is_busy() {
        app.push_notice(
            NoticeLevel::Info,
            "A task is already running. Wait for it to finish.",
        );
    } else if agent.is_none() {
        app.push_notice(
            NoticeLevel::Warning,
            "No active agent available for review.",
        );
    } else if app.active_pending_interaction().is_some() {
        app.push_notice(
            NoticeLevel::Warning,
            "Resolve the pending interaction before starting a review.",
        );
    } else {
        start_capture(app, Arc::new(git_diff::GitDiffCapture));
    }
}

fn start_capture(app: &mut TuiApp, capture: Arc<dyn DiffCapture>) {
    let cwd = PathBuf::from(&app.snapshot.cwd);
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        // Keep the task channel open for the lifetime of the preparation task.
        let _sender = sender;
        let result = capture.capture(&cwd).await.map(|diff| {
            if diff.text.trim().is_empty() && !diff.truncated {
                return ReviewPreparation::Clean;
            }
            let mut lines = diff.text.lines();
            let preview = lines.by_ref().take(600).collect::<Vec<_>>().join("\n");
            let truncated = diff.truncated || lines.next().is_some();
            let suffix = if truncated {
                "\n\n(Full diff truncated by the size or line limit; use tools to inspect the remaining changes.)"
            } else {
                ""
            };
            ReviewPreparation::Prompt(format!(
                "Review the following code changes:\n\n```diff\n{preview}\n```{suffix}"
            ))
        });
        TaskCompletion::ReviewPrepared { result }
    });
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::ReviewPreparation,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: Some(Arc::new(AtomicBool::new(false))),
        query_control: None,
    });
    app.push_notice(NoticeLevel::Info, "Collecting local changes for review.");
    app.set_runtime_phase(
        RuntimePhase::LocalCommand,
        Some("collecting changes".into()),
    );
}

pub(super) fn finish(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    result: anyhow::Result<ReviewPreparation>,
) {
    // A permission change admitted during preparation applies before the query
    // consumes the agent, just as it does before other task continuations.
    if let Some(mode) = app.pending_permission_mode.take() {
        super::permissions::request_permission_mode(app, agent_slot, mode);
    }
    match result {
        Ok(ReviewPreparation::Prompt(prompt)) => {
            if let Some(agent) = agent_slot.take() {
                super::tasks::start_review_task(app, prompt, agent);
            } else {
                app.push_notice(
                    NoticeLevel::Warning,
                    "Review could not start: the runtime agent is unavailable.",
                );
                app.set_runtime_phase(RuntimePhase::Failed, Some("review unavailable".into()));
            }
        }
        Ok(ReviewPreparation::Clean) => {
            app.push_notice(
                NoticeLevel::Info,
                "No staged or unstaged changes to review.",
            );
            app.set_runtime_phase(RuntimePhase::Idle, Some("no changes to review".into()));
        }
        Ok(ReviewPreparation::Cancelled) => {
            app.push_notice(NoticeLevel::Info, "Review preparation cancelled.");
            app.set_runtime_phase(
                RuntimePhase::Idle,
                Some("review preparation cancelled".into()),
            );
        }
        Err(error) => {
            log::warn!("Review preparation failed: {error:#}");
            app.push_notice(
                NoticeLevel::Error,
                format!("Could not collect changes for review: {error:#}"),
            );
            app.set_runtime_phase(
                RuntimePhase::Failed,
                Some("review preparation failed".into()),
            );
        }
    }
}

#[cfg(test)]
mod tests;
