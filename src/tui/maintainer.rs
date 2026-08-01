//! TUI maintainer — owns all render state and applies agent-output events.
//!
//! The agent task sends `TuiEvent`s through an mpsc channel. The maintainer
//! is the sole consumer: it drains events, mutates `TuiApp`, and exposes a
//! ready/dirty signal so the event loop knows when to redraw.
//!
//! This is an incremental extraction of the apply logic already present in
//! `finish_running_task_if_ready` / `apply_tui_event`; no behavior change.

use super::state::{TuiApp, TuiEvent};
use crate::agent::Agent;
use crate::runtime_client::RuntimeClient;

/// Owns all TUI render state and applies agent-output events to it.
pub(super) struct TuiMaintainer {
    app: TuiApp,
    runtime: RuntimeClient,
    /// Set to true every time an event is applied and the screen should repaint.
    pub(super) needs_redraw: bool,
}

impl TuiMaintainer {
    pub(super) fn new(app: TuiApp, runtime: RuntimeClient) -> Self {
        Self {
            app,
            runtime,
            needs_redraw: true,
        }
    }

    pub(super) fn app(&self) -> &TuiApp {
        &self.app
    }

    pub(super) fn app_mut(&mut self) -> &mut TuiApp {
        &mut self.app
    }

    pub(super) fn agent(&self) -> Option<&Agent> {
        self.runtime.agent()
    }

    /// Split borrow so callers that need independent `&mut TuiApp` and
    /// `&mut Option<Agent>` can still use them while the maintainer
    /// owns both.
    pub(super) fn split_mut(&mut self) -> (&mut TuiApp, &mut Option<Agent>) {
        (&mut self.app, self.runtime.agent_mut())
    }

    /// Drain pending agent events from the running task, apply them, and
    /// finalize the task if it has completed.
    pub(super) async fn poll_agent_task(&mut self) -> anyhow::Result<()> {
        // Delegate to the existing logic; move it to TuiMaintainer in the next commit.
        super::runtime::tasks::finish_running_task_if_ready(
            &mut self.app,
            self.runtime.agent_mut(),
        )
        .await?;
        self.needs_redraw = true;
        Ok(())
    }

    /// Wait for the next runtime event without a timer-driven try-receive.
    pub(super) async fn wait_for_agent_event(&mut self) -> Option<TuiEvent> {
        let Some(task) = self.app.bottom_pane.running_task.as_mut() else {
            std::future::pending().await
        };
        task.receiver.recv().await
    }

    pub(super) fn apply_agent_event(&mut self, event: TuiEvent) {
        super::runtime::apply_tui_event(&mut self.app, event);
        self.needs_redraw = true;
    }

    /// Sync snapshot from the active agent (must be called at the top of the event loop).
    pub(super) fn sync_snapshot(&mut self) {
        let (app, runtime) = (&mut self.app, &self.runtime);
        if let Some(agent_ref) = runtime.agent() {
            app.sync_snapshot(agent_ref);
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
