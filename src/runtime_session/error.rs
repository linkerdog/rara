use super::{RuntimeTurnId, RuntimeTurnOutcome};

/// Typed failures returned by the session command boundary.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeSessionError {
    #[error("runtime session is already executing turn {active_turn}")]
    Busy { active_turn: RuntimeTurnId },
    #[error("runtime session has no active turn")]
    NotRunning,
    #[error("runtime turn target {expected} does not match active turn {active}")]
    StaleTurn {
        expected: RuntimeTurnId,
        active: RuntimeTurnId,
    },
    #[error("runtime turn {active_turn} already has a different stop request")]
    StopInProgress { active_turn: RuntimeTurnId },
    #[error("runtime source request is invalid or targets a different session")]
    InvalidSource,
    #[error("runtime source scope, layer, or lifetime is unsupported")]
    UnsupportedSource,
    #[error("runtime source capacity exceeded")]
    SourceCapacity,
    #[error("runtime host already contains session {0}")]
    AlreadyExists(super::RuntimeSessionId),
    #[error("runtime session command queue is full")]
    Overloaded,
    #[error("runtime session is closing or closed")]
    Closed,
    #[error("runtime session cleanup failed")]
    ShutdownFailed,
    #[error("runtime session actor stopped before acknowledging the command")]
    ActorStopped,
    #[error("runtime turn was cancelled")]
    Cancelled { outcome: RuntimeTurnOutcome },
    #[error("runtime turn was interrupted")]
    Interrupted { outcome: RuntimeTurnOutcome },
    #[error("runtime event subscriber lagged by {0} event(s)")]
    EventLagged(u64),
    #[error(
        "runtime event replay requires resync: requested after {requested}, oldest available is {oldest_available}, latest is {latest}"
    )]
    ResyncRequired {
        requested: u64,
        oldest_available: u64,
        latest: u64,
    },
    #[error("runtime turn failed: {message}")]
    Execution {
        message: String,
        outcome: RuntimeTurnOutcome,
    },
}

impl RuntimeSessionError {
    /// Return partial evidence retained for cancellation, interruption, or execution failure.
    pub fn turn_outcome(&self) -> Option<&RuntimeTurnOutcome> {
        match self {
            Self::Cancelled { outcome }
            | Self::Interrupted { outcome }
            | Self::Execution { outcome, .. } => Some(outcome),
            _ => None,
        }
    }
}
