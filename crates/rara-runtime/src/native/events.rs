use rara_core::tool::ToolProgressEvent;

use crate::{RuntimeSessionId, RuntimeTurnId, SequencedEvent};

/// Ordered host-facing events. Native applications may project richer events
/// through their own adapter while sharing the same owner and event log.
#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    Session(SessionEvent),
    Assistant(AssistantEvent),
    Tool(ToolEvent),
    Status(String),
    Error(String),
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    TurnStarted,
    TurnFinished,
    TurnCancelled,
    TurnInterrupted,
    TurnFailed {
        message: String,
    },
    RuntimeState {
        snapshot: crate::RuntimeSessionSnapshot,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistantEvent {
    Text(String),
    Delta(String),
    ThinkingDelta(String),
}

#[derive(Clone, Debug)]
pub enum ToolEvent {
    Use {
        call_id: String,
        name: String,
        input: serde_json::Value,
    },
    Progress {
        call_id: String,
        name: String,
        event: ToolProgressEvent,
    },
    Result {
        call_id: String,
        name: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Clone, Debug)]
pub struct RuntimeControlEvent {
    pub event_id: String,
    pub session_id: RuntimeSessionId,
    pub turn_id: Option<RuntimeTurnId>,
    pub sequence: u64,
    pub event: RuntimeEvent,
}

impl SequencedEvent for RuntimeControlEvent {
    fn sequence(&self) -> u64 {
        self.sequence
    }
}
