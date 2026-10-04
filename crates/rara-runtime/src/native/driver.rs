use std::sync::{Arc, atomic::AtomicBool};

use async_trait::async_trait;
use rara_core::{llm::types::Message, observation::QueryReport};
use rara_observability::InferenceAgent;
use serde::{Deserialize, Serialize};

use crate::{RuntimeSessionError, RuntimeTurnId, SessionSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnStopKind {
    Cancel,
    Interrupt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputDiscardReason {
    Cancelled,
    Interrupted,
    Shutdown,
    Superseded,
}

/// Identifies the turn that owns an adapter's pending interaction.
pub trait PendingInteraction: Clone + Send + Sync + 'static {
    fn turn_id(&self) -> &RuntimeTurnId;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingDisposition {
    None,
    Answered,
    Superseded,
}

/// Trusted execution metadata assigned only after session admission.
pub struct TurnContext {
    pub turn_id: RuntimeTurnId,
    pub cancellation: Arc<AtomicBool>,
    pub inference_agent: InferenceAgent,
    pub pending: PendingDisposition,
}

pub struct CompletedTurn<P> {
    pub query_report: QueryReport,
    pub transcript: Vec<Message>,
    pub pending_input: Option<P>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnFinishReason {
    Completed,
    AwaitingInput,
}

impl TurnFinishReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::AwaitingInput => "awaiting_input",
        }
    }
}

/// Lifecycle events are published by the owner after the relevant state change.
pub enum SessionLifecycle<P> {
    TurnStarted {
        turn_id: RuntimeTurnId,
    },
    InputAnswered {
        waiting_turn: RuntimeTurnId,
        turn_id: RuntimeTurnId,
    },
    InputDiscarded {
        waiting_turn: RuntimeTurnId,
        reason: InputDiscardReason,
    },
    InputRequested {
        pending: P,
    },
    TurnFinished {
        turn_id: RuntimeTurnId,
        reason: TurnFinishReason,
    },
    TurnStopped {
        turn_id: RuntimeTurnId,
        kind: TurnStopKind,
    },
    TurnFailed {
        turn_id: RuntimeTurnId,
        message: String,
    },
    RuntimeState {
        snapshot: SessionSnapshot<P>,
    },
}

/// Owns one admitted executor until all its work and cleanup actually return.
#[async_trait]
pub trait SessionTurn: Send + 'static {
    async fn run(&mut self) -> anyhow::Result<()>;
}

/// Supplies session-scoped policy and resources to the shared session owner.
///
/// A successful `begin_turn` transfers the executor into `Turn`; `complete_turn`
/// must restore it before returning. Implementations publish events in order and
/// must not spawn a second session scheduling loop or detach cleanup from a turn.
#[async_trait]
pub trait SessionDriver: Send + 'static {
    type Input: Send + 'static;
    type Control: Send + 'static;
    type Pending: PendingInteraction;
    type Turn: SessionTurn;

    fn validate_input(
        &self,
        input: &Self::Input,
        pending: Option<&Self::Pending>,
    ) -> Result<(), RuntimeSessionError>;
    fn is_answer(input: &Self::Input) -> bool;
    fn begin_turn(
        &mut self,
        input: Self::Input,
        context: TurnContext,
    ) -> Result<Self::Turn, RuntimeSessionError>;
    fn complete_turn(
        &mut self,
        turn: Self::Turn,
        discard: Option<InputDiscardReason>,
    ) -> CompletedTurn<Self::Pending>;
    async fn after_turn(&mut self) {}
    fn transcript(&self) -> Result<Vec<Message>, RuntimeSessionError>;
    fn replace_transcript(&mut self, transcript: Vec<Message>) -> Result<(), RuntimeSessionError>;
    async fn control(&mut self, control: Self::Control) -> Result<(), RuntimeSessionError>;
    fn discard_pending(&mut self) -> Result<(), RuntimeSessionError>;
    fn cancel_descendants(&self) -> anyhow::Result<()>;
    fn begin_shutdown(&mut self) -> anyhow::Result<()>;
    async fn shutdown(&mut self) -> anyhow::Result<()>;
    fn publish(&self, event: SessionLifecycle<Self::Pending>);
    fn current_sequence(&self) -> u64;
}
