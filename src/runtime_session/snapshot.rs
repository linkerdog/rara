use serde::{Deserialize, Serialize};

use super::{RuntimePendingInput, RuntimeSessionId, RuntimeTurnId};

/// Observable lifecycle state for one runtime session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum RuntimeSessionPhase {
    Idle,
    AwaitingInput { turn_id: RuntimeTurnId },
    Running { turn_id: RuntimeTurnId },
    Cancelling { turn_id: RuntimeTurnId },
    Closing,
    Closed,
}

/// Point-in-time session state paired with the ordered event cursor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSessionSnapshot {
    pub session_id: RuntimeSessionId,
    pub phase: RuntimeSessionPhase,
    pub generation: u64,
    pub last_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_input: Option<RuntimePendingInput>,
}
