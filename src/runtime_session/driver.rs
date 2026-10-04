use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use rara_runtime::{
    CompletedTurn, InputDiscardReason, PendingDisposition, SessionDriver, SessionLifecycle,
    SessionTurn, TurnContext, TurnStopKind,
};

use super::command::NativeControl;
use super::input::TurnInput;
use super::{RuntimePendingInput, RuntimeSessionError, RuntimeSessionId, RuntimeTurnId};
use crate::agent::{Agent, AgentEvent, AgentOutputMode};
use crate::llm::Message;
use crate::memory_lifecycle::MemorySyncReason;
use crate::protocol_sources::{PromptSourceError, SkillSourceError};
use crate::runtime_client::RuntimeClient;
use crate::runtime_control::{InputEvent, RuntimeEvent, RuntimeProvenance, SessionEvent};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tools::agent::AgentTreeControl;

pub(super) struct NativeInput {
    pub input: TurnInput,
    pub output_mode: AgentOutputMode,
}

pub(super) struct NativeSessionDriver {
    id: RuntimeSessionId,
    client: RuntimeClient,
    agent_tree_control: Arc<AgentTreeControl>,
}

pub(super) struct NativeTurn {
    agent: Agent,
    input: Option<NativeInput>,
    event_bus: Arc<RuntimeEventBus>,
    session_id: RuntimeSessionId,
    turn_id: RuntimeTurnId,
}

impl NativeSessionDriver {
    pub(super) fn new(
        id: RuntimeSessionId,
        client: RuntimeClient,
        agent_tree_control: Arc<AgentTreeControl>,
    ) -> Self {
        Self {
            id,
            client,
            agent_tree_control,
        }
    }

    fn agent_mut(&mut self) -> Result<&mut Agent, RuntimeSessionError> {
        self.client
            .agent_mut()
            .as_mut()
            .ok_or(RuntimeSessionError::ActorStopped)
    }

    fn agent_event(&self, turn_id: &RuntimeTurnId, event: AgentEvent) {
        self.client.event_bus.send_with_turn(
            event,
            RuntimeProvenance::runtime(Some(self.id.to_string())),
            Some(turn_id.as_str()),
        );
    }

    fn control_event(&self, turn_id: Option<&RuntimeTurnId>, event: RuntimeEvent) {
        self.client.event_bus.publish_control_with_turn(
            event,
            RuntimeProvenance::runtime(Some(self.id.to_string())),
            turn_id.map(RuntimeTurnId::as_str),
        );
    }

    fn terminal(&self, turn_id: &RuntimeTurnId, reason: &str, event: SessionEvent) {
        self.client.event_bus.publish_raw(AgentEvent::AgentStop {
            reason: reason.to_owned(),
        });
        self.control_event(Some(turn_id), RuntimeEvent::Session(event));
    }
}

#[async_trait]
impl SessionTurn for NativeTurn {
    async fn run(&mut self) -> anyhow::Result<()> {
        let NativeInput { input, output_mode } = self
            .input
            .take()
            .context("native session turn executed twice")?;
        let event_bus = self.event_bus.clone();
        let provenance = RuntimeProvenance::runtime(Some(self.session_id.to_string()));
        let turn_id = self.turn_id.clone();
        input
            .execute(&mut self.agent, output_mode, move |event| {
                event_bus.send_with_turn(event, provenance.clone(), Some(turn_id.as_str()));
            })
            .await
    }
}

#[async_trait]
impl SessionDriver for NativeSessionDriver {
    type Input = NativeInput;
    type Control = NativeControl;
    type Pending = RuntimePendingInput;
    type Turn = NativeTurn;

    fn validate_input(
        &self,
        input: &NativeInput,
        pending: Option<&RuntimePendingInput>,
    ) -> Result<(), RuntimeSessionError> {
        input.input.validate(pending)
    }

    fn is_answer(input: &NativeInput) -> bool {
        input.input.is_answer()
    }

    fn begin_turn(
        &mut self,
        input: NativeInput,
        context: TurnContext,
    ) -> Result<NativeTurn, RuntimeSessionError> {
        let mut agent = self
            .client
            .agent_mut()
            .take()
            .ok_or(RuntimeSessionError::ActorStopped)?;
        if context.pending == PendingDisposition::Superseded {
            agent.discard_pending_interactions();
        }
        agent.set_cancellation_token(Some(context.cancellation));
        agent.set_runtime_turn_id(Some(context.turn_id.to_string()));
        agent.pending_inference_agent = Some(context.inference_agent);
        Ok(NativeTurn {
            agent,
            input: Some(input),
            event_bus: self.client.event_bus.clone(),
            session_id: self.id.clone(),
            turn_id: context.turn_id,
        })
    }

    fn complete_turn(
        &mut self,
        mut turn: NativeTurn,
        discard: Option<InputDiscardReason>,
    ) -> CompletedTurn<RuntimePendingInput> {
        turn.agent.set_cancellation_token(None);
        turn.agent.set_runtime_turn_id(None);
        let pending_input = RuntimePendingInput::from_agent(turn.turn_id, &turn.agent);
        if discard.is_some() {
            turn.agent.discard_pending_interactions();
        }
        let completed = CompletedTurn {
            query_report: turn.agent.last_query_report.clone(),
            transcript: turn.agent.history.clone(),
            pending_input,
        };
        *self.client.agent_mut() = Some(turn.agent);
        completed
    }

    async fn after_turn(&mut self) {
        if let Some(agent) = self.client.agent() {
            self.client
                .capture_memory(agent, MemorySyncReason::TurnIdle)
                .await;
        }
    }

    fn transcript(&self) -> Result<Vec<Message>, RuntimeSessionError> {
        self.client
            .agent()
            .map(|agent| agent.history.clone())
            .ok_or(RuntimeSessionError::ActorStopped)
    }

    fn replace_transcript(&mut self, transcript: Vec<Message>) -> Result<(), RuntimeSessionError> {
        self.agent_mut()?.replace_history(transcript);
        Ok(())
    }

    async fn control(&mut self, control: NativeControl) -> Result<(), RuntimeSessionError> {
        match control {
            NativeControl::ReplaceBackend { backend } => self.agent_mut()?.llm_backend = backend,
            NativeControl::SetMaxTurns { max_turns } => self.agent_mut()?.set_max_turns(max_turns),
            NativeControl::DisableTools => self.agent_mut()?.tool_manager.retain(|_| false),
            NativeControl::DisableExtensionExecution => {
                self.agent_mut()?.disable_extension_execution()
            }
            NativeControl::SetFullAccess { enabled } => {
                self.agent_mut()?.set_full_access_mode(enabled)
            }
            NativeControl::PromptSource {
                request,
                provenance,
            } => {
                self.client
                    .prompt_source_registry
                    .handle_control_with_provenance(&request, provenance)
                    .await
                    .map_err(|error| match error {
                        PromptSourceError::Invalid => RuntimeSessionError::InvalidSource,
                        PromptSourceError::Unsupported => RuntimeSessionError::UnsupportedSource,
                        PromptSourceError::Capacity => RuntimeSessionError::SourceCapacity,
                    })?;
            }
            NativeControl::SkillSource {
                request,
                provenance,
            } => {
                if !self
                    .client
                    .agent()
                    .is_some_and(|agent| agent.tool_manager.get_tool("skill").is_some())
                {
                    return Err(RuntimeSessionError::UnsupportedSource);
                }
                self.client
                    .skill_source_registry
                    .handle_control_with_provenance(&request, provenance)
                    .await
                    .map_err(|error| match error {
                        SkillSourceError::Invalid => RuntimeSessionError::InvalidSource,
                        SkillSourceError::Unsupported => RuntimeSessionError::UnsupportedSource,
                        SkillSourceError::Capacity => RuntimeSessionError::SourceCapacity,
                        SkillSourceError::Unavailable => RuntimeSessionError::SourceUnavailable,
                    })?;
            }
        }
        Ok(())
    }

    fn discard_pending(&mut self) -> Result<(), RuntimeSessionError> {
        self.agent_mut()?.discard_pending_interactions();
        Ok(())
    }

    fn cancel_descendants(&self) -> anyhow::Result<()> {
        self.agent_tree_control.cancel_running()?;
        Ok(())
    }

    fn begin_shutdown(&mut self) -> anyhow::Result<()> {
        self.agent_tree_control.begin_shutdown()?;
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        let result = self.agent_tree_control.shutdown().await;
        self.client.drain_memory().await;
        result.map_err(Into::into)
    }

    fn current_sequence(&self) -> u64 {
        self.client.event_bus.current_sequence()
    }

    fn publish(&self, event: SessionLifecycle<RuntimePendingInput>) {
        match event {
            SessionLifecycle::TurnStarted { turn_id } => {
                self.agent_event(&turn_id, AgentEvent::AgentStart)
            }
            SessionLifecycle::InputAnswered {
                waiting_turn,
                turn_id,
            } => self.control_event(
                Some(&turn_id),
                RuntimeEvent::Input(InputEvent::Answered {
                    waiting_turn: waiting_turn.to_string(),
                }),
            ),
            SessionLifecycle::InputDiscarded {
                waiting_turn,
                reason,
            } => self.control_event(
                Some(&waiting_turn),
                RuntimeEvent::Input(InputEvent::Discarded {
                    waiting_turn: waiting_turn.to_string(),
                    reason,
                }),
            ),
            SessionLifecycle::InputRequested { pending } => {
                let turn_id = pending.turn_id.clone();
                self.control_event(
                    Some(&turn_id),
                    RuntimeEvent::Input(InputEvent::Requested {
                        pending: Box::new(pending),
                    }),
                );
            }
            SessionLifecycle::TurnFinished { turn_id, reason } => self.terminal(
                &turn_id,
                reason.as_str(),
                SessionEvent::TurnFinished {
                    reason: Some(reason.as_str().to_owned()),
                },
            ),
            SessionLifecycle::TurnStopped { turn_id, kind } => match kind {
                TurnStopKind::Cancel => {
                    self.terminal(&turn_id, "cancelled", SessionEvent::TurnCancelled)
                }
                TurnStopKind::Interrupt => {
                    self.terminal(&turn_id, "interrupted", SessionEvent::TurnInterrupted)
                }
            },
            SessionLifecycle::TurnFailed { turn_id, message } => {
                self.agent_event(
                    &turn_id,
                    AgentEvent::AgentError {
                        message: message.clone(),
                        recoverable: false,
                    },
                );
                self.terminal(
                    &turn_id,
                    "failed",
                    SessionEvent::TurnFailed { reason: message },
                );
            }
            SessionLifecycle::RuntimeState { snapshot } => self.control_event(
                None,
                RuntimeEvent::Session(SessionEvent::RuntimeState { snapshot }),
            ),
        }
    }
}
