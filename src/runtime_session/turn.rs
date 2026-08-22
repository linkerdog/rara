use tokio::sync::oneshot;

use super::{RuntimeSessionError, RuntimeTurnId};
use crate::llm::Message;
use crate::model_observation::QueryReport;

/// Completed observations for one submitted turn.
#[derive(Clone, Debug)]
pub struct RuntimeTurnOutcome {
    /// Stable identity assigned when the turn was accepted.
    pub turn_id: RuntimeTurnId,
    /// Provider usage, cache, timing, and request-fingerprint observations.
    pub query_report: QueryReport,
    /// Model-visible transcript after all completed work in this turn.
    pub transcript: Vec<Message>,
}

/// Accepted turn handle that can be awaited independently of the session.
#[derive(Debug)]
pub struct RuntimeTurn {
    turn_id: RuntimeTurnId,
    completion: oneshot::Receiver<Result<RuntimeTurnOutcome, RuntimeSessionError>>,
}

impl RuntimeTurn {
    pub(crate) fn new(
        turn_id: RuntimeTurnId,
        completion: oneshot::Receiver<Result<RuntimeTurnOutcome, RuntimeSessionError>>,
    ) -> Self {
        Self {
            turn_id,
            completion,
        }
    }

    /// Return this turn's stable identity.
    pub fn id(&self) -> &RuntimeTurnId {
        &self.turn_id
    }

    /// Wait for the terminal turn outcome.
    pub async fn wait(self) -> Result<RuntimeTurnOutcome, RuntimeSessionError> {
        self.completion
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }
}
