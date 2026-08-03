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
use super::event_dispatch::dispatch_event_with_runtime;
use super::input_control;
use super::runtime_port::{
    InProcessRuntimeClientPort, RuntimeClientPort, RuntimeCommand, RuntimeEventStream,
    RuntimeMaintenanceCommand, RuntimeProjectionEvent,
};
use super::state::{TaskCompletion, TuiApp};
use crate::agent::Agent;
use crate::oauth::OAuthManager;
use crate::runtime_client::RuntimeClient;
use crate::runtime_control::{InputControlRequest, SessionControlRequest};

pub(super) enum RuntimeActivity {
    Event(Option<RuntimeProjectionEvent>),
    Command(Option<RuntimeCommand>),
    Completed(Box<Result<TaskCompletion, tokio::task::JoinError>>),
}

/// Owns TUI presentation state and applies runtime projections to it.
pub(super) struct TuiController {
    app: TuiApp,
    runtime: RuntimeClient,
    runtime_port: InProcessRuntimeClientPort,
    runtime_events: RuntimeEventStream,
    runtime_commands: tokio::sync::mpsc::UnboundedReceiver<RuntimeCommand>,
    /// Set to true every time an event is applied and the screen should repaint.
    pub(super) needs_redraw: bool,
}

impl TuiController {
    pub(super) fn new(app: TuiApp, runtime: RuntimeClient) -> Self {
        let (runtime_port, runtime_commands) = InProcessRuntimeClientPort::new(
            runtime.event_bus.clone(),
            std::sync::Arc::new(std::sync::RwLock::new(app.snapshot.clone())),
        );
        let runtime_events = runtime_port.subscribe();
        Self {
            app,
            runtime,
            runtime_port,
            runtime_events,
            runtime_commands,
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
        event: AppEvent,
        oauth_manager: &std::sync::Arc<OAuthManager>,
    ) -> anyhow::Result<bool> {
        let (app, runtime, runtime_port) = (&mut self.app, &mut self.runtime, &self.runtime_port);
        dispatch_event_with_runtime(event, app, runtime.agent_mut(), oauth_manager, runtime_port)
            .await
    }

    pub(super) async fn send_runtime_command(&self, command: RuntimeCommand) -> anyhow::Result<()> {
        self.runtime_port.send(command).await
    }

    pub(super) async fn apply_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> anyhow::Result<()> {
        match command {
            RuntimeCommand::Input(InputControlRequest::SubmitUserPrompt { prompt }) => {
                input_control::submit_user_prompt(&mut self.app, self.runtime.agent_mut(), prompt);
            }
            RuntimeCommand::Input(InputControlRequest::SubmitFollowUp { prompt }) => {
                input_control::submit_follow_up(&mut self.app, prompt, false);
            }
            RuntimeCommand::Input(InputControlRequest::AnswerPendingInput { answer }) => {
                if let Some(agent) = self.runtime.agent_mut().take() {
                    input_control::answer_pending_input(
                        &mut self.app,
                        self.runtime.agent_mut(),
                        agent,
                        answer,
                    );
                } else {
                    self.app
                        .push_notice("Request input is still preparing. Try again.");
                }
            }
            RuntimeCommand::Input(InputControlRequest::AnswerPlanApproval {
                decision,
                feedback,
            }) => {
                input_control::answer_plan_approval_with_feedback(
                    &mut self.app,
                    self.runtime.agent_mut(),
                    decision,
                    feedback,
                );
            }
            RuntimeCommand::Input(InputControlRequest::AnswerShellApproval { decision }) => {
                input_control::answer_shell_approval(
                    &mut self.app,
                    self.runtime.agent_mut(),
                    decision,
                );
            }
            RuntimeCommand::Session(SessionControlRequest::CancelCurrentTurn) => {
                input_control::handle_session_control(
                    &mut self.app,
                    SessionControlRequest::CancelCurrentTurn,
                );
            }
            RuntimeCommand::Maintenance(RuntimeMaintenanceCommand::Compact) => {
                if let Some(agent) = self.runtime.agent_mut().take() {
                    super::runtime::start_compact_task(&mut self.app, agent);
                } else {
                    self.app
                        .push_notice("No active agent available for compaction.");
                }
            }
            RuntimeCommand::Maintenance(RuntimeMaintenanceCommand::Rebuild) => {
                super::runtime::start_rebuild_task(&mut self.app);
            }
            RuntimeCommand::Maintenance(RuntimeMaintenanceCommand::LoadDeepSeekModels) => {
                super::runtime::start_deepseek_model_list_task(&mut self.app);
            }
            RuntimeCommand::Maintenance(RuntimeMaintenanceCommand::LoadKimiModels) => {
                super::runtime::start_kimi_model_list_task(&mut self.app);
            }
            command => self.app.push_notice(format!(
                "Runtime command is not handled by the in-process TUI: {command:?}"
            )),
        }
        self.needs_redraw = true;
        Ok(())
    }

    pub(super) fn agent(&self) -> Option<&Agent> {
        self.runtime.agent()
    }

    /// Split borrow so callers that need independent `&mut TuiApp` and
    /// `&mut Option<Agent>` can still use them while the controller
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
        super::runtime::tasks::finish_running_task_if_ready_from_runtime_port(
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

    pub(super) fn apply_runtime_event(&mut self, event: RuntimeProjectionEvent) {
        match event {
            RuntimeProjectionEvent::Runtime(event) => {
                super::runtime::apply_tui_event(
                    &mut self.app,
                    super::state::TuiEvent::Runtime(event),
                );
            }
            RuntimeProjectionEvent::Snapshot(snapshot) => self.app.snapshot = *snapshot,
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
        self.needs_redraw = true;
    }

    pub(super) fn publish_snapshot_projection(&self) {
        if let Ok(mut snapshot) = self.runtime_port.snapshot_store().write() {
            *snapshot = self.app.snapshot.clone();
        }
    }

    /// Sync snapshot from the active agent (must be called at the top of the event loop).
    pub(super) async fn sync_snapshot(&mut self) -> anyhow::Result<()> {
        if let Some(agent_ref) = self.runtime.agent() {
            self.app.sync_snapshot(agent_ref);
            if let Ok(mut snapshot) = self.runtime_port.snapshot_store().write() {
                *snapshot = self.app.snapshot.clone();
            }
        }
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

    use super::{RuntimeActivity, RuntimeCommand, select_runtime_activity};
    use crate::runtime_control::SessionControlRequest;
    use crate::tui::runtime_port::{RuntimeEventStream, RuntimeProjectionEvent};

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
            crate::tui::state::TaskCompletion::KimiModels {
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
