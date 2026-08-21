//! TUI controller — owns presentation state and applies runtime projections.
//!
//! The agent task sends `TuiEvent`s through an mpsc channel. The controller
//! is the sole consumer: it drains events, mutates `TuiApp`, and exposes a
//! ready/dirty signal so the event loop knows when to redraw.
//!
//! This is an incremental extraction of the apply logic already present in
//! `finish_running_task_if_ready` / `apply_tui_event`. The compatibility task
//! bridge remains here until it is replaced by `RuntimeClientPort`.

use futures::StreamExt;

use super::app_event::AppEvent;
use super::runtime::RuntimeCommandProcessor;
use super::runtime_port::{
    RuntimeClientPort, RuntimeCommand, RuntimeEventStream, RuntimeProjectionEvent,
    accept_runtime_event,
};
use super::state::{TaskCompletion, TuiApp};
use crate::oauth::OAuthManager;
use crate::runtime_control::{ErrorEvent, RuntimeEvent, SessionEvent};

pub(super) enum RuntimeActivity {
    Event(Option<RuntimeProjectionEvent>),
    Command(Option<RuntimeCommand>),
    Completed(Box<Result<TaskCompletion, tokio::task::JoinError>>),
}

#[derive(Default)]
struct QueryCompletionBarrier {
    terminal_event_seen: bool,
    pending_completion: Option<Box<Result<TaskCompletion, tokio::task::JoinError>>>,
}

impl QueryCompletionBarrier {
    fn observe_terminal_event(&mut self) {
        self.terminal_event_seen = true;
    }

    fn defer(&mut self, completion: Box<Result<TaskCompletion, tokio::task::JoinError>>) {
        let task_join_failed = completion.is_err();
        if self.pending_completion.replace(completion).is_some() {
            log::warn!("replaced a pending query completion before its terminal event");
        }
        if task_join_failed {
            // A failed task no longer has a producer that can publish a
            // terminal runtime event. Treat the join failure as that boundary
            // so the TUI can surface the task error instead of waiting forever.
            self.terminal_event_seen = true;
        }
    }

    fn take_ready(&mut self) -> Option<Box<Result<TaskCompletion, tokio::task::JoinError>>> {
        if !self.terminal_event_seen {
            return None;
        }
        let completion = self.pending_completion.take()?;
        self.terminal_event_seen = false;
        Some(completion)
    }

    fn has_pending_completion(&self) -> bool {
        self.pending_completion.is_some()
    }

    fn reset(&mut self) {
        self.terminal_event_seen = false;
        self.pending_completion = None;
    }
}

/// Owns TUI presentation state and applies runtime projections to it.
pub(super) struct TuiController {
    app: TuiApp,
    runtime_port: std::sync::Arc<dyn RuntimeClientPort>,
    runtime_events: RuntimeEventStream,
    runtime_commands: tokio::sync::mpsc::UnboundedReceiver<RuntimeCommand>,
    last_runtime_event: Option<(Option<String>, u64, String)>,
    query_completion_barrier: QueryCompletionBarrier,
    /// Set to true every time an event is applied and the screen should repaint.
    pub(super) needs_redraw: bool,
}

impl TuiController {
    pub(super) fn new(
        app: TuiApp,
        runtime_port: std::sync::Arc<dyn RuntimeClientPort>,
        runtime_commands: tokio::sync::mpsc::UnboundedReceiver<RuntimeCommand>,
    ) -> Self {
        let runtime_events = runtime_port.subscribe();
        Self {
            app,
            runtime_port,
            runtime_events,
            runtime_commands,
            last_runtime_event: None,
            query_completion_barrier: QueryCompletionBarrier::default(),
            needs_redraw: true,
        }
    }

    pub(super) fn app(&self) -> &TuiApp {
        &self.app
    }

    pub(super) fn app_mut(&mut self) -> &mut TuiApp {
        &mut self.app
    }

    pub(super) async fn dispatch_event(
        &mut self,
        processor: &mut RuntimeCommandProcessor,
        event: AppEvent,
        oauth_manager: &std::sync::Arc<OAuthManager>,
    ) -> anyhow::Result<bool> {
        processor
            .dispatch_event(&mut self.app, event, oauth_manager, &*self.runtime_port)
            .await
    }

    pub(super) async fn send_runtime_command(&self, command: RuntimeCommand) -> anyhow::Result<()> {
        self.runtime_port.send(command).await
    }

    pub(super) async fn apply_runtime_command(
        &mut self,
        processor: &mut RuntimeCommandProcessor,
        command: RuntimeCommand,
    ) -> anyhow::Result<()> {
        processor.apply_command(&mut self.app, command).await?;
        self.needs_redraw = true;
        Ok(())
    }

    /// Defer query completion until the ordered runtime stream reaches a
    /// terminal event. Maintenance tasks do not emit turn lifecycle events and
    /// continue to complete directly from their join handle.
    pub(super) async fn receive_runtime_task_completion(
        &mut self,
        processor: &mut RuntimeCommandProcessor,
        completion: Box<Result<TaskCompletion, tokio::task::JoinError>>,
    ) -> anyhow::Result<bool> {
        if self.running_task_is_query() {
            self.query_completion_barrier.defer(completion);
            return self.complete_query_if_ready(processor).await;
        }
        processor.complete(&mut self.app, completion).await?;
        self.query_completion_barrier.reset();
        self.needs_redraw = true;
        Ok(true)
    }

    pub(super) async fn complete_query_if_ready(
        &mut self,
        processor: &mut RuntimeCommandProcessor,
    ) -> anyhow::Result<bool> {
        let Some(completion) = self.query_completion_barrier.take_ready() else {
            return Ok(false);
        };
        processor.complete(&mut self.app, completion).await?;
        self.needs_redraw = true;
        Ok(true)
    }

    fn running_task_is_query(&self) -> bool {
        self.app
            .bottom_pane
            .running_task
            .as_ref()
            .is_some_and(|task| matches!(&task.kind, super::state::TaskKind::Query))
    }

    /// Wait for the next runtime event or task completion without polling.
    pub(super) async fn wait_for_runtime_activity(&mut self) -> RuntimeActivity {
        if self.query_completion_barrier.has_pending_completion() {
            let activity = tokio::select! {
                event = self.runtime_events.next() => RuntimeActivity::Event(event),
                command = self.runtime_commands.recv() => RuntimeActivity::Command(command),
            };
            return match activity {
                RuntimeActivity::Event(None) => {
                    RuntimeActivity::Event(Some(RuntimeProjectionEvent::Disconnected {
                        reason: "runtime event stream closed before turn completion".into(),
                    }))
                }
                activity => activity,
            };
        }
        if let Some(task) = self.app.bottom_pane.running_task.as_mut() {
            select_runtime_activity(
                &mut self.runtime_events,
                &mut self.runtime_commands,
                &mut task.handle,
            )
            .await
        } else {
            tokio::select! {
                event = self.runtime_events.next() => RuntimeActivity::Event(event),
                command = self.runtime_commands.recv() => RuntimeActivity::Command(command),
            }
        }
    }

    pub(super) fn apply_runtime_event(&mut self, event: RuntimeProjectionEvent) -> bool {
        let terminal_event = is_terminal_projection_event(&event);
        match event {
            RuntimeProjectionEvent::Runtime(event) => {
                if !accept_runtime_event(&mut self.last_runtime_event, &event) {
                    return false;
                }
                super::runtime::apply_tui_event(
                    &mut self.app,
                    super::state::TuiEvent::Runtime(event),
                );
            }
            RuntimeProjectionEvent::Snapshot(snapshot) => {
                self.app.snapshot = *snapshot;
                let catalogs = self.app.snapshot.model_catalogs.clone();
                self.app.apply_model_catalog_snapshots(&catalogs);
            }
            RuntimeProjectionEvent::Completed { reason } => {
                self.app
                    .set_runtime_phase(super::state::RuntimePhase::Idle, reason);
            }
            RuntimeProjectionEvent::Disconnected { reason } => {
                self.app
                    .set_runtime_phase(super::state::RuntimePhase::Failed, Some(reason));
            }
            RuntimeProjectionEvent::Reconnected => {
                self.app
                    .set_runtime_phase(super::state::RuntimePhase::Idle, None);
            }
        }
        if terminal_event && self.running_task_is_query() {
            self.query_completion_barrier.observe_terminal_event();
        }
        self.needs_redraw = true;
        true
    }

    pub(super) fn publish_snapshot_projection(&self) {
        self.runtime_port
            .publish_snapshot(self.app.snapshot.clone());
    }

    /// Sync snapshot from the active agent (must be called at the top of the event loop).
    pub(super) async fn sync_snapshot(
        &mut self,
        processor: &RuntimeCommandProcessor,
    ) -> anyhow::Result<()> {
        processor.sync_snapshot(&mut self.app);
        self.app.snapshot = self.runtime_port.snapshot().await?;
        Ok(())
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

fn is_terminal_projection_event(event: &RuntimeProjectionEvent) -> bool {
    match event {
        RuntimeProjectionEvent::Runtime(event) => matches!(
            &event.event,
            RuntimeEvent::Session(
                SessionEvent::TurnFinished { .. }
                    | SessionEvent::TurnFailed { .. }
                    | SessionEvent::TurnCancelled
                    | SessionEvent::TurnInterrupted
            ) | RuntimeEvent::Error(ErrorEvent::RuntimeError {
                recoverable: false,
                ..
            })
        ),
        RuntimeProjectionEvent::Completed { .. } | RuntimeProjectionEvent::Disconnected { .. } => {
            true
        }
        RuntimeProjectionEvent::Snapshot(_) | RuntimeProjectionEvent::Reconnected => false,
    }
}

async fn select_runtime_activity(
    runtime_events: &mut RuntimeEventStream,
    runtime_commands: &mut tokio::sync::mpsc::UnboundedReceiver<RuntimeCommand>,
    handle: &mut tokio::task::JoinHandle<TaskCompletion>,
) -> RuntimeActivity {
    let activity = tokio::select! {
        event = runtime_events.next() => RuntimeActivity::Event(event),
        command = runtime_commands.recv() => RuntimeActivity::Command(command),
        result = &mut *handle => RuntimeActivity::Completed(Box::new(result)),
    };
    match activity {
        RuntimeActivity::Event(Some(event)) => RuntimeActivity::Event(Some(event)),
        RuntimeActivity::Event(None) => RuntimeActivity::Completed(Box::new(handle.await)),
        RuntimeActivity::Command(command) => RuntimeActivity::Command(command),
        RuntimeActivity::Completed(result) => RuntimeActivity::Completed(result),
    }
}

#[cfg(test)]
mod tests {
    use futures::stream;

    use super::{
        QueryCompletionBarrier, RuntimeActivity, RuntimeCommand, is_terminal_projection_event,
        select_runtime_activity,
    };
    use crate::runtime_control::{
        RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, SessionControlRequest, SessionEvent,
    };
    use crate::tui::runtime_port::{RuntimeEventStream, RuntimeProjectionEvent};
    use crate::tui::state::TaskCompletion;

    fn test_completion() -> Box<Result<TaskCompletion, tokio::task::JoinError>> {
        Box::new(Ok(TaskCompletion::ModelCatalog {
            provider: rara_provider_catalog::ModelCatalogProvider::Kimi,
            result: Ok(Vec::new()),
        }))
    }

    fn runtime_projection(event: RuntimeEvent) -> RuntimeProjectionEvent {
        RuntimeProjectionEvent::Runtime(Box::new(RuntimeControlEvent {
            event_id: "test-event".into(),
            provenance: RuntimeProvenance::local_tui("test-session"),
            turn_id: None,
            sequence: 1,
            event,
        }))
    }

    #[test]
    fn query_completion_waits_for_terminal_event_when_task_finishes_first() {
        let mut barrier = QueryCompletionBarrier::default();

        barrier.defer(test_completion());
        assert!(barrier.take_ready().is_none());

        barrier.observe_terminal_event();
        assert!(barrier.take_ready().is_some());
        assert!(barrier.take_ready().is_none());
    }

    #[test]
    fn query_completion_finishes_when_terminal_event_arrives_first() {
        let mut barrier = QueryCompletionBarrier::default();

        barrier.observe_terminal_event();
        barrier.defer(test_completion());

        assert!(barrier.take_ready().is_some());
        assert!(barrier.take_ready().is_none());
    }

    #[tokio::test]
    async fn query_join_failure_does_not_wait_for_an_event_that_cannot_arrive() {
        let handle = tokio::spawn(async { std::future::pending::<TaskCompletion>().await });
        handle.abort();
        let completion = handle.await;
        assert!(completion.is_err());

        let mut barrier = QueryCompletionBarrier::default();
        barrier.defer(Box::new(completion));

        assert!(barrier.take_ready().is_some());
    }

    #[test]
    fn turn_finished_is_a_terminal_projection_event() {
        assert!(is_terminal_projection_event(&runtime_projection(
            RuntimeEvent::Session(SessionEvent::TurnFinished {
                reason: Some("turn complete".into()),
            }),
        )));
        assert!(!is_terminal_projection_event(&runtime_projection(
            RuntimeEvent::Session(SessionEvent::TurnStarted),
        )));
    }

    #[tokio::test]
    async fn runtime_mux_wakes_on_event() {
        let mut handle = tokio::spawn(async {
            std::future::pending::<crate::tui::state::TaskCompletion>().await
        });
        let mut runtime_events: RuntimeEventStream =
            Box::pin(stream::once(async { RuntimeProjectionEvent::Reconnected }));
        let (_sender, mut runtime_commands) = tokio::sync::mpsc::unbounded_channel();

        let activity =
            select_runtime_activity(&mut runtime_events, &mut runtime_commands, &mut handle).await;
        assert!(matches!(
            activity,
            RuntimeActivity::Event(Some(RuntimeProjectionEvent::Reconnected))
        ));
        handle.abort();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn runtime_mux_waits_for_completion_after_receiver_closes() {
        let mut handle = tokio::spawn(async {
            panic!("completion failure");
            #[allow(unreachable_code)]
            crate::tui::state::TaskCompletion::ModelCatalog {
                provider: rara_provider_catalog::ModelCatalogProvider::Kimi,
                result: Ok(Vec::new()),
            }
        });
        let mut runtime_events: RuntimeEventStream = Box::pin(stream::empty());
        let (_sender, mut runtime_commands) = tokio::sync::mpsc::unbounded_channel();

        let activity =
            select_runtime_activity(&mut runtime_events, &mut runtime_commands, &mut handle).await;
        assert!(matches!(
            activity,
            RuntimeActivity::Completed(error)
                if error.as_ref().as_ref().is_err_and(|error| error.is_panic())
        ));
    }

    #[tokio::test]
    async fn runtime_mux_wakes_on_command_without_running_task() {
        let (sender, mut commands) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(RuntimeCommand::Session(
                SessionControlRequest::CancelCurrentTurn,
            ))
            .expect("command send");
        let mut runtime_events: RuntimeEventStream = Box::pin(stream::pending());
        let mut handle = tokio::spawn(async {
            std::future::pending::<crate::tui::state::TaskCompletion>().await
        });

        let activity =
            select_runtime_activity(&mut runtime_events, &mut commands, &mut handle).await;
        assert!(matches!(
            activity,
            RuntimeActivity::Command(Some(RuntimeCommand::Session(
                SessionControlRequest::CancelCurrentTurn
            )))
        ));
        handle.abort();
        let _ = handle.await;
    }
}
