//! TUI maintainer — owns all render state and applies agent-output events.
//!
//! The agent task sends `TuiEvent`s through an mpsc channel. The maintainer
//! is the sole consumer: it drains events, mutates `TuiApp`, and exposes a
//! ready/dirty signal so the event loop knows when to redraw.
//!
//! This is an incremental extraction of the apply logic already present in
//! `finish_running_task_if_ready` / `apply_tui_event`.

use super::state::{TaskCompletion, TuiApp, TuiEvent};
use crate::agent::Agent;
use crate::runtime_client::RuntimeClient;

pub(super) enum RuntimeActivity {
    Event(Option<TuiEvent>),
    Completed(Box<Result<TaskCompletion, tokio::task::JoinError>>),
}

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
    pub(super) async fn complete_runtime_task(
        &mut self,
        completion: Box<Result<TaskCompletion, tokio::task::JoinError>>,
    ) -> anyhow::Result<()> {
        super::runtime::tasks::finish_running_task_if_ready_with_completion(
            &mut self.app,
            self.runtime.agent_mut(),
            Some(*completion),
        )
        .await?;
        self.needs_redraw = true;
        Ok(())
    }

    /// Wait for the next runtime event or task completion without polling.
    pub(super) async fn wait_for_runtime_activity(&mut self) -> RuntimeActivity {
        let Some(task) = self.app.bottom_pane.running_task.as_mut() else {
            std::future::pending().await
        };
        select_runtime_activity(&mut task.receiver, &mut task.handle).await
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
    pub(super) async fn poll_repo_context(&mut self) -> bool {
        let before = (self.app.repo_slug.clone(), self.app.current_pr_url.clone());
        self.app.finish_repo_context_task_if_ready().await;
        before.0 != self.app.repo_slug || before.1 != self.app.current_pr_url
    }
}

async fn select_runtime_activity(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<TuiEvent>,
    handle: &mut tokio::task::JoinHandle<TaskCompletion>,
) -> RuntimeActivity {
    let activity = tokio::select! {
        event = receiver.recv() => RuntimeActivity::Event(event),
        result = &mut *handle => RuntimeActivity::Completed(Box::new(result)),
    };
    match activity {
        RuntimeActivity::Event(Some(event)) => RuntimeActivity::Event(Some(event)),
        RuntimeActivity::Event(None) => RuntimeActivity::Completed(Box::new(handle.await)),
        RuntimeActivity::Completed(result) => RuntimeActivity::Completed(result),
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::{RuntimeActivity, select_runtime_activity};
    use crate::tui::state::TuiEvent;

    #[tokio::test]
    async fn runtime_mux_wakes_on_event() {
        let (sender, mut receiver) = mpsc::unbounded_channel::<TuiEvent>();
        sender
            .send(TuiEvent::Transcript {
                role: "Status",
                message: "ready".into(),
            })
            .expect("send event");
        let mut handle = tokio::spawn(async {
            std::future::pending::<crate::tui::state::TaskCompletion>().await
        });

        let activity = select_runtime_activity(&mut receiver, &mut handle).await;
        assert!(matches!(
            activity,
            RuntimeActivity::Event(Some(TuiEvent::Transcript { message, .. }))
                if message == "ready"
        ));
        handle.abort();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn runtime_mux_waits_for_completion_after_receiver_closes() {
        let (sender, mut receiver) = mpsc::unbounded_channel::<TuiEvent>();
        drop(sender);
        let mut handle = tokio::spawn(async {
            panic!("completion failure");
            #[allow(unreachable_code)]
            crate::tui::state::TaskCompletion::KimiModels {
                result: Ok(Vec::new()),
            }
        });

        let activity = select_runtime_activity(&mut receiver, &mut handle).await;
        assert!(matches!(
            activity,
            RuntimeActivity::Completed(error)
                if error.as_ref().as_ref().is_err_and(|error| error.is_panic())
        ));
    }
}
