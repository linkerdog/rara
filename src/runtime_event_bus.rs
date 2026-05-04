use tokio::sync::broadcast;

use crate::agent::AgentEvent;

/// Shared runtime event bus that mirrors `AgentEvent` for non-TUI
/// subscribers (ACP, Wire, rollout log).  The TUI continues to receive
/// events through the separate `convert_agent_event → mpsc` path.
///
/// Built on a `tokio::sync::broadcast` channel so subscribers receive every
/// event without the bus needing to know about them ahead of time.  Slow
/// subscribers will see `broadcast::error::Lagged` and should decide whether
/// to catch up or reconnect.
#[derive(Clone, Debug)]
pub struct RuntimeEventBus {
    sender: broadcast::Sender<AgentEvent>,
}

impl RuntimeEventBus {
    /// Create a new bus with a fixed ring-buffer capacity.  When the buffer
    /// is full the oldest event is dropped for the slowest subscriber.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Push an event to all active subscribers.  Returns the number of
    /// subscribers that received the event (may be 0).
    pub fn send(&self, event: AgentEvent) -> usize {
        self.sender.send(event).unwrap_or(0)
    }

    /// Create a new receiver that will see all future events.  Past events
    /// are not replayed.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.sender.subscribe()
    }

    /// Return the number of active subscribers.
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}
