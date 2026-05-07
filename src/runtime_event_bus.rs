use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::broadcast;

use crate::agent::AgentEvent;
use crate::runtime_control::{
    RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, wrap_agent_event,
};

/// Shared runtime event bus for raw agent events and structured protocol
/// subscribers. The TUI continues to receive events through the separate
/// `convert_agent_event → mpsc` path.
///
/// Built on a `tokio::sync::broadcast` channel so subscribers receive every
/// event without the bus needing to know about them ahead of time.  Slow
/// subscribers will see `broadcast::error::Lagged` and should decide whether
/// to catch up or reconnect.
#[derive(Clone, Debug)]
pub struct RuntimeEventBus {
    raw_sender: broadcast::Sender<AgentEvent>,
    control_sender: broadcast::Sender<RuntimeControlEvent>,
    next_sequence: Arc<AtomicU64>,
}

impl RuntimeEventBus {
    /// Create a new bus with a fixed ring-buffer capacity.  When the buffer
    /// is full the oldest event is dropped for the slowest subscriber.
    pub fn new(capacity: usize) -> Self {
        let (raw_sender, _) = broadcast::channel(capacity);
        let (control_sender, _) = broadcast::channel(capacity);
        Self {
            raw_sender,
            control_sender,
            next_sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Push an event to all active subscribers with runtime provenance.
    /// Returns the number of raw and structured subscribers that received the
    /// event (may be 0).
    pub fn send(&self, event: AgentEvent) -> usize {
        self.send_with_provenance(event, RuntimeProvenance::runtime(None))
    }

    /// Push an event with explicit provenance for protocol-ready subscribers.
    pub fn send_with_provenance(&self, event: AgentEvent, provenance: RuntimeProvenance) -> usize {
        let raw_receivers = self.raw_sender.receiver_count();
        let control_receivers = self.control_sender.receiver_count();

        match (raw_receivers > 0, control_receivers > 0) {
            (false, false) => 0,
            (true, false) => self.raw_sender.send(event).unwrap_or(0),
            (false, true) => {
                let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
                let event_id = format!("evt-{sequence:016x}");
                let control_event = wrap_agent_event(event_id, sequence, provenance, event);
                self.control_sender.send(control_event).unwrap_or(0)
            }
            (true, true) => {
                let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
                let event_id = format!("evt-{sequence:016x}");
                let control_event = wrap_agent_event(event_id, sequence, provenance, event.clone());
                let raw_count = self.raw_sender.send(event).unwrap_or(0);
                let control_count = self.control_sender.send(control_event).unwrap_or(0);
                raw_count + control_count
            }
        }
    }

    /// Create a new receiver that will see all future events.  Past events
    /// are not replayed.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.raw_sender.subscribe()
    }

    /// Create a structured receiver for ACP/Wire/appserver adapters.
    ///
    /// Past events are not replayed. Use the embedded sequence and event id to
    /// preserve stream order and adapter acknowledgements.
    pub fn subscribe_control(&self) -> broadcast::Receiver<RuntimeControlEvent> {
        self.control_sender.subscribe()
    }

    /// Return the number of active subscribers.
    pub fn receiver_count(&self) -> usize {
        self.raw_sender.receiver_count() + self.control_sender.receiver_count()
    }

    /// Publish a structured `RuntimeEvent` on the control bus without wrapping
    /// an `AgentEvent`. Used for protocol-native events (MCP, hooks, etc.) that
    /// originate from the runtime itself rather than from agent execution.
    pub fn publish_control(&self, event: RuntimeEvent) -> usize {
        if self.control_sender.receiver_count() == 0 {
            return 0;
        }
        let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let event_id = format!("ctl-{sequence:016x}");
        let control_event = RuntimeControlEvent {
            event_id,
            provenance: RuntimeProvenance::runtime(None),
            sequence,
            event,
        };
        self.control_sender.send(control_event).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn control_subscriber_receives_wrapped_runtime_events() {
        let bus = RuntimeEventBus::new(8);
        let mut control = bus.subscribe_control();

        assert_eq!(
            bus.send_with_provenance(
                AgentEvent::Status("ready".to_string()),
                RuntimeProvenance::local_tui("session-1"),
            ),
            1
        );

        let event = control.try_recv().expect("control event");
        assert_eq!(event.event_id, "evt-0000000000000001");
        assert_eq!(event.sequence, 1);
        assert_eq!(event.provenance.session_id.as_deref(), Some("session-1"));
        assert_eq!(
            serde_json::to_value(event.event).unwrap(),
            json!({
                "type": "session",
                "payload": {
                    "type": "status",
                    "payload": {
                        "message": "ready"
                    }
                }
            })
        );
    }

    #[test]
    fn raw_and_control_subscribers_share_sequence_source() {
        let bus = RuntimeEventBus::new(8);
        let mut raw = bus.subscribe();
        let mut control = bus.subscribe_control();

        assert_eq!(bus.receiver_count(), 2);
        assert_eq!(bus.send(AgentEvent::AssistantDelta("hello".to_string())), 2);

        assert!(matches!(
            raw.try_recv().expect("raw event"),
            AgentEvent::AssistantDelta(delta) if delta == "hello"
        ));
        let event = control.try_recv().expect("control event");
        assert_eq!(event.sequence, 1);
        assert_eq!(
            event.provenance.controller,
            crate::runtime_control::RuntimeControllerKind::Runtime
        );
    }

    #[test]
    fn unsubscribed_events_do_not_advance_control_sequence() {
        let bus = RuntimeEventBus::new(8);

        assert_eq!(bus.send(AgentEvent::Status("ignored".to_string())), 0);

        let mut raw = bus.subscribe();
        assert_eq!(bus.send(AgentEvent::Status("raw only".to_string())), 1);
        assert!(matches!(
            raw.try_recv().expect("raw event"),
            AgentEvent::Status(message) if message == "raw only"
        ));
        drop(raw);

        let mut control = bus.subscribe_control();
        assert_eq!(bus.send(AgentEvent::Status("first control".to_string())), 1);

        let event = control.try_recv().expect("control event");
        assert_eq!(event.sequence, 1);
        assert_eq!(event.event_id, "evt-0000000000000001");
    }
}
