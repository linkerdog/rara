use rara_core::llm::types::Message;
use rara_observability::InferenceAgent;
use tokio::sync::oneshot;

use crate::{RuntimeSessionError, RuntimeTurnId, RuntimeTurnOutcome, SessionDriver, TurnStopKind};

pub(crate) type TurnResultSender = oneshot::Sender<Result<RuntimeTurnOutcome, RuntimeSessionError>>;

pub(crate) enum SessionCommand<D: SessionDriver> {
    StartTurn {
        turn_id: RuntimeTurnId,
        input: D::Input,
        inference_agent: InferenceAgent,
        accepted: oneshot::Sender<Result<(), RuntimeSessionError>>,
        completed: TurnResultSender,
    },
    StopTurn {
        expected_turn: Option<RuntimeTurnId>,
        kind: TurnStopKind,
        response: oneshot::Sender<Result<RuntimeTurnId, RuntimeSessionError>>,
    },
    Control {
        control: D::Control,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    GetTranscript {
        response: oneshot::Sender<Result<Vec<Message>, RuntimeSessionError>>,
    },
    ReplaceTranscript {
        transcript: Vec<Message>,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    QueryState {
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    Shutdown {
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
}
