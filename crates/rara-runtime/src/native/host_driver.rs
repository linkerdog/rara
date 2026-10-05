use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rara_core::{llm::backend::LlmBackend, llm::types::Message, tool::ToolManager};
use serde::{Deserialize, Serialize};

use super::host_turn::HostTurn;
use crate::{
    CompletedTurn, EventLog, InputDiscardReason, PendingInteraction, RuntimeControlEvent,
    RuntimeEvent, RuntimeSessionError, RuntimeSessionId, RuntimeTurnId, SessionDriver,
    SessionEvent, SessionLifecycle, TurnContext, TurnStopKind,
};

/// Host tools own their asynchronous approval interactions; no native pending
/// input payload is introduced by the host execution adapter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoPendingInput {}

impl PendingInteraction for NoPendingInput {
    fn turn_id(&self) -> &RuntimeTurnId {
        match *self {}
    }
}

pub(crate) struct HostState {
    pub backend: Arc<dyn LlmBackend>,
    pub tools: ToolManager,
    pub transcript: Vec<Message>,
    pub workspace_root: PathBuf,
    pub system_prompt: Option<String>,
    pub max_turns: Option<usize>,
}

pub(crate) enum HostControl {
    ReplaceBackend(Arc<dyn LlmBackend>),
    SetMaxTurns(Option<usize>),
}

#[derive(Clone)]
pub(crate) struct HostEvents {
    pub session_id: RuntimeSessionId,
    pub log: Arc<EventLog<RuntimeControlEvent>>,
}

impl HostEvents {
    pub fn publish(&self, turn_id: Option<&RuntimeTurnId>, event: RuntimeEvent) {
        self.log.publish(
            |sequence| RuntimeControlEvent {
                event_id: format!("evt-{sequence:016x}"),
                session_id: self.session_id.clone(),
                turn_id: turn_id.cloned(),
                sequence,
                event,
            },
            |_| {},
        );
    }
}

pub(crate) struct HostDriver {
    pub state: Option<HostState>,
    pub events: HostEvents,
}

#[async_trait]
impl SessionDriver for HostDriver {
    type Input = String;
    type Control = HostControl;
    type Pending = NoPendingInput;
    type Turn = HostTurn;

    fn validate_input(
        &self,
        _input: &String,
        pending: Option<&NoPendingInput>,
    ) -> Result<(), RuntimeSessionError> {
        if let Some(pending) = pending {
            match *pending {}
        }
        Ok(())
    }
    fn is_answer(_input: &String) -> bool {
        false
    }

    fn begin_turn(
        &mut self,
        prompt: String,
        context: TurnContext,
    ) -> Result<HostTurn, RuntimeSessionError> {
        let state = self.state.take().ok_or(RuntimeSessionError::ActorStopped)?;
        Ok(HostTurn::new(state, self.events.clone(), prompt, context))
    }

    fn complete_turn(
        &mut self,
        turn: HostTurn,
        _discard: Option<InputDiscardReason>,
    ) -> CompletedTurn<NoPendingInput> {
        let completed = CompletedTurn {
            query_report: turn.report,
            transcript: turn.state.transcript.clone(),
            pending_input: None,
        };
        self.state = Some(turn.state);
        completed
    }

    fn transcript(&self) -> Result<Vec<Message>, RuntimeSessionError> {
        Ok(self
            .state
            .as_ref()
            .ok_or(RuntimeSessionError::ActorStopped)?
            .transcript
            .clone())
    }
    fn replace_transcript(&mut self, transcript: Vec<Message>) -> Result<(), RuntimeSessionError> {
        self.state
            .as_mut()
            .ok_or(RuntimeSessionError::ActorStopped)?
            .transcript = transcript;
        Ok(())
    }
    async fn control(&mut self, control: HostControl) -> Result<(), RuntimeSessionError> {
        let state = self
            .state
            .as_mut()
            .ok_or(RuntimeSessionError::ActorStopped)?;
        match control {
            HostControl::ReplaceBackend(backend) => state.backend = backend,
            HostControl::SetMaxTurns(limit) => state.max_turns = limit,
        }
        Ok(())
    }
    fn discard_pending(&mut self) -> Result<(), RuntimeSessionError> {
        Ok(())
    }
    fn cancel_descendants(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn begin_shutdown(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn shutdown(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn current_sequence(&self) -> u64 {
        self.events.log.current_sequence()
    }

    fn publish(&self, event: SessionLifecycle<NoPendingInput>) {
        let (turn_id, event) = match event {
            SessionLifecycle::TurnStarted { turn_id } => (Some(turn_id), SessionEvent::TurnStarted),
            SessionLifecycle::TurnFinished { turn_id, .. } => {
                (Some(turn_id), SessionEvent::TurnFinished)
            }
            SessionLifecycle::TurnStopped { turn_id, kind } => (
                Some(turn_id),
                match kind {
                    TurnStopKind::Cancel => SessionEvent::TurnCancelled,
                    TurnStopKind::Interrupt => SessionEvent::TurnInterrupted,
                },
            ),
            SessionLifecycle::TurnFailed { turn_id, message } => {
                self.events
                    .publish(Some(&turn_id), RuntimeEvent::Error(message.clone()));
                (Some(turn_id), SessionEvent::TurnFailed { message })
            }
            SessionLifecycle::RuntimeState { snapshot } => {
                (None, SessionEvent::RuntimeState { snapshot })
            }
            SessionLifecycle::InputRequested { pending } => match pending {},
            SessionLifecycle::InputAnswered { .. } | SessionLifecycle::InputDiscarded { .. } => {
                log::warn!("host session received an unexpected pending-input lifecycle event");
                return;
            }
        };
        self.events
            .publish(turn_id.as_ref(), RuntimeEvent::Session(event));
    }
}
