use std::sync::Arc;

use super::super::app_event::AppEvent;
use super::super::event_dispatch::dispatch_event_with_runtime;
use super::super::input_control;
use super::super::runtime_port::{RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand};
use super::super::state::{TaskCompletion, TuiApp};
use crate::agent::Agent;
use crate::memory_lifecycle::MemorySyncReason;
use crate::oauth::OAuthManager;
use crate::runtime_client::RuntimeClient;
use crate::runtime_client::RuntimeTaskServices;
use crate::runtime_control::{InputControlRequest, SessionControlRequest};

/// Owns session runtime execution and applies typed commands to the runtime.
///
/// The TUI controller passes presentation state into this processor but does
/// not retain the session `Agent`, registries, or runtime replacement state.
pub(crate) struct RuntimeCommandProcessor {
    runtime: RuntimeClient,
}

impl RuntimeCommandProcessor {
    pub(crate) fn new(runtime: RuntimeClient) -> Self {
        Self { runtime }
    }

    pub(crate) fn event_bus(&self) -> Arc<crate::runtime_event_bus::RuntimeEventBus> {
        self.runtime.event_bus.clone()
    }

    pub(crate) fn agent(&self) -> Option<&Agent> {
        self.runtime.agent()
    }

    pub(crate) fn agent_mut(&mut self) -> &mut Option<Agent> {
        self.runtime.agent_mut()
    }

    pub(crate) fn session_id(&self) -> Option<String> {
        self.agent()
            .map(|agent| agent.session_id.clone())
            .filter(|id| !id.is_empty())
    }

    pub(crate) async fn dispatch_event(
        &mut self,
        app: &mut TuiApp,
        event: AppEvent,
        oauth_manager: &Arc<OAuthManager>,
        runtime_port: &dyn RuntimeClientPort,
    ) -> anyhow::Result<bool> {
        dispatch_event_with_runtime(event, app, self.agent_mut(), oauth_manager, runtime_port).await
    }

    pub(crate) async fn apply_command(
        &mut self,
        app: &mut TuiApp,
        command: RuntimeCommand,
    ) -> anyhow::Result<()> {
        match command {
            RuntimeCommand::Input(InputControlRequest::SubmitUserPrompt { prompt }) => {
                let services = self.runtime.task_services();
                let agent_slot = self.runtime.agent_mut();
                input_control::submit_user_prompt_with_services(
                    app,
                    agent_slot,
                    prompt,
                    Some(services),
                );
            }
            RuntimeCommand::Input(InputControlRequest::SubmitFollowUp { prompt }) => {
                input_control::submit_follow_up(app, prompt, false);
            }
            RuntimeCommand::Input(InputControlRequest::AnswerPendingInput { answer }) => {
                let services = self.runtime.task_services();
                let agent_slot = self.runtime.agent_mut();
                if let Some(agent) = agent_slot.take() {
                    input_control::answer_pending_input_with_services(
                        app,
                        agent_slot,
                        agent,
                        answer,
                        Some(services),
                    );
                } else {
                    app.push_notice("Request input is still preparing. Try again.");
                }
            }
            RuntimeCommand::Input(InputControlRequest::AnswerPlanApproval {
                decision,
                feedback,
            }) => {
                let services = self.runtime.task_services();
                let agent_slot = self.runtime.agent_mut();
                input_control::answer_plan_approval_with_feedback_and_services(
                    app,
                    agent_slot,
                    decision,
                    feedback,
                    Some(services),
                );
            }
            RuntimeCommand::Input(InputControlRequest::AnswerShellApproval { decision }) => {
                let services = self.runtime.task_services();
                let agent_slot = self.runtime.agent_mut();
                input_control::answer_shell_approval_with_services(
                    app,
                    agent_slot,
                    decision,
                    Some(services),
                );
            }
            RuntimeCommand::Session(SessionControlRequest::CancelCurrentTurn) => {
                input_control::handle_session_control(
                    app,
                    SessionControlRequest::CancelCurrentTurn,
                );
            }
            RuntimeCommand::Maintenance(RuntimeMaintenanceCommand::Compact) => {
                if let Some(agent) = self.runtime.agent() {
                    self.runtime
                        .capture_memory(agent, MemorySyncReason::Compaction)
                        .await;
                }
                if let Some(agent) = self.agent_mut().take() {
                    super::start_compact_task(app, agent);
                } else {
                    app.push_notice("No active agent available for compaction.");
                }
            }
            RuntimeCommand::Maintenance(RuntimeMaintenanceCommand::Rebuild) => {
                super::start_rebuild_task(app);
            }
            RuntimeCommand::Maintenance(RuntimeMaintenanceCommand::RefreshModelCatalog(
                provider,
            )) => super::start_model_catalog_task(app, provider),
            command => app.push_notice(format!(
                "Runtime command is not handled by the in-process processor: {command:?}"
            )),
        }
        Ok(())
    }

    pub(crate) async fn complete(
        &mut self,
        app: &mut TuiApp,
        completion: Box<Result<TaskCompletion, tokio::task::JoinError>>,
    ) -> anyhow::Result<()> {
        let mut services = self.runtime.task_services();
        let agent_slot = self.runtime.agent_mut();
        super::tasks::finish_running_task_if_ready_from_runtime_port(
            app,
            agent_slot,
            Some(*completion),
            Some(&mut services),
        )
        .await?;
        self.runtime.update_task_services(&services);
        if let Some(agent) = self.runtime.agent() {
            crate::auto_memory::maybe_auto_memory(app, agent);
            self.runtime
                .capture_memory(agent, MemorySyncReason::TurnIdle)
                .await;
        }
        Ok(())
    }

    pub(crate) async fn drain_memory(&self) {
        self.runtime.drain_memory().await;
        let _ = crate::auto_memory::drain_auto_memory_for_shutdown().await;
    }

    pub(crate) fn sync_snapshot(&self, app: &mut TuiApp) {
        if let Some(agent) = self.agent() {
            app.apply_runtime_snapshot(agent, self.runtime.extension_snapshot());
        }
    }
}
