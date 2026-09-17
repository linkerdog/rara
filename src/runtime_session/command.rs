use std::sync::Arc;

use tokio::sync::oneshot;

use super::input::TurnInput;
use super::{RuntimeSessionError, RuntimeTurnId, RuntimeTurnOutcome};
use crate::agent::AgentOutputMode;
use crate::llm::{LlmBackend, Message};
use crate::runtime_control::{
    PromptSourceControlRequest, RuntimeProvenance, SkillSourceControlRequest,
};

pub(super) type TurnResultSender = oneshot::Sender<Result<RuntimeTurnOutcome, RuntimeSessionError>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnStopKind {
    Cancel,
    Interrupt,
}

pub(super) enum SessionCommand {
    StartTurn {
        turn_id: RuntimeTurnId,
        input: TurnInput,
        output_mode: AgentOutputMode,
        accepted: oneshot::Sender<Result<(), RuntimeSessionError>>,
        completed: TurnResultSender,
        inference_agent: rara_observability::InferenceAgent,
    },
    StopTurn {
        expected_turn: Option<RuntimeTurnId>,
        kind: TurnStopKind,
        response: oneshot::Sender<Result<RuntimeTurnId, RuntimeSessionError>>,
    },
    ReplaceBackend {
        backend: Arc<dyn LlmBackend>,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    SetMaxTurns {
        max_turns: usize,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    DisableTools {
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    DisableExtensionExecution {
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    SetFullAccess {
        enabled: bool,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    GetTranscript {
        response: oneshot::Sender<Result<Vec<Message>, RuntimeSessionError>>,
    },
    ReplaceTranscript {
        transcript: Vec<Message>,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    PromptSource {
        request: PromptSourceControlRequest,
        provenance: RuntimeProvenance,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    Shutdown {
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    SkillSource {
        request: SkillSourceControlRequest,
        provenance: RuntimeProvenance,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
}
