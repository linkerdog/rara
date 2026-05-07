//! TUI maintainer — owns all render state and applies agent-output events.
//!
//! The agent task sends `TuiEvent`s through an mpsc channel. The maintainer
//! is the sole consumer: it drains events, mutates `TuiApp`, and exposes a
//! ready/dirty signal so the event loop knows when to redraw.
//!
//! This is an incremental extraction of the apply logic already present in
//! `finish_running_task_if_ready` / `apply_tui_event`; no behavior change.

use crate::agent::Agent;
use super::state::TuiApp;

/// Owns all TUI render state and applies agent-output events to it.
pub(super) struct TuiMaintainer {
    app: TuiApp,
    agent: Option<Agent>,
    /// Set to true every time an event is applied and the screen should repaint.
    pub(super) needs_redraw: bool,
}

impl TuiMaintainer {
    pub(super) fn new(app: TuiApp, agent: Option<Agent>) -> Self {
        Self { app, agent, needs_redraw: true }
    }

    pub(super) fn app(&self) -> &TuiApp {
        &self.app
    }

    pub(super) fn app_mut(&mut self) -> &mut TuiApp {
        &mut self.app
    }

    pub(super) fn agent(&self) -> Option<&Agent> {
        self.agent.as_ref()
    }

    pub(super) fn agent_mut(&mut self) -> &mut Option<Agent> {
        &mut self.agent
    }

    /// Drain pending agent events from the running task, apply them, and
    /// finalize the task if it has completed.
    pub(super) async fn poll_agent_task(&mut self) -> anyhow::Result<()> {
        // Delegate to the existing logic; move it to TuiMaintainer in the next commit.
        super::runtime::tasks::finish_running_task_if_ready(
            &mut self.app,
            &mut self.agent,
        )
        .await?;
        self.needs_redraw = true;
        Ok(())
    }

    /// Sync snapshot from the active agent (must be called at the top of the event loop).
    pub(super) fn sync_snapshot(&mut self) {
        if let Some(agent_ref) = self.agent.as_ref() {
            self.app.sync_snapshot(agent_ref);
        }
    }

    /// Start repo context detection (async side task).
    pub(super) fn start_repo_context_detection(&mut self) {
        self.app.start_repo_context_detection();
    }

    /// Poll and finish repo context task if ready.
    pub(super) async fn poll_repo_context(&mut self) {
        self.app.finish_repo_context_task_if_ready().await;
    }
}
