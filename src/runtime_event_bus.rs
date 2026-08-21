use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
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
/// subscribers. Presentation consumers subscribe to the structured control
/// stream and do not reconstruct semantics from transcript text.
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
    tool_identities: Arc<Mutex<ToolIdentityTracker>>,
}

#[derive(Debug, Default)]
struct ToolIdentityTracker {
    pending: HashMap<(Option<String>, String), VecDeque<String>>,
}

impl ToolIdentityTracker {
    fn project(&mut self, event: &mut RuntimeControlEvent) {
        let session_id = event.provenance.session_id.clone();
        let event_id = event.event_id.clone();
        let RuntimeEvent::Tool(tool_event) = &mut event.event else {
            return;
        };

        match tool_event {
            crate::runtime_control::ToolEvent::Use { call_id, name, .. } => {
                if call_id.is_none() {
                    let id = format!("tool-{event_id}");
                    *call_id = Some(id.clone());
                    self.pending
                        .entry((session_id, name.clone()))
                        .or_default()
                        .push_back(id);
                }
            }
            crate::runtime_control::ToolEvent::Result { call_id, name, .. } => {
                if call_id.is_none() {
                    let key = (session_id, name.clone());
                    if let Some(ids) = self.pending.get_mut(&key) {
                        *call_id = ids.pop_front();
                        if ids.is_empty() {
                            self.pending.remove(&key);
                        }
                    }
                }
            }
            crate::runtime_control::ToolEvent::Progress { call_id, name, .. } => {
                if call_id.is_none() {
                    *call_id = self
                        .pending
                        .get(&(session_id, name.clone()))
                        .and_then(VecDeque::front)
                        .cloned();
                }
            }
        }
    }
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
            tool_identities: Arc::new(Mutex::new(ToolIdentityTracker::default())),
        }
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
                let mut control_event = wrap_agent_event(event_id, sequence, provenance, event);
                self.project_tool_identity(&mut control_event);
                self.control_sender.send(control_event).unwrap_or(0)
            }
            (true, true) => {
                let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
                let event_id = format!("evt-{sequence:016x}");
                let mut control_event =
                    wrap_agent_event(event_id, sequence, provenance, event.clone());
                self.project_tool_identity(&mut control_event);
                let raw_count = self.raw_sender.send(event).unwrap_or(0);
                let control_count = self.control_sender.send(control_event).unwrap_or(0);
                raw_count + control_count
            }
        }
    }

    fn project_tool_identity(&self, event: &mut RuntimeControlEvent) {
        match self.tool_identities.lock() {
            Ok(mut tracker) => tracker.project(event),
            Err(poisoned) => {
                log::warn!("runtime tool identity tracker lock was poisoned; recovering");
                poisoned.into_inner().project(event);
            }
        }
    }

    /// Publish only to legacy raw-event consumers.
    ///
    /// The in-process control-plane dispatcher publishes its own structured
    /// lifecycle events. This path keeps hooks and legacy consumers informed
    /// without duplicating those lifecycle boundaries on the ordered control
    /// stream.
    pub(crate) fn publish_raw(&self, event: AgentEvent) -> usize {
        if self.raw_sender.receiver_count() == 0 {
            return 0;
        }
        self.raw_sender.send(event).unwrap_or(0)
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
    /// Reserved for external structured event subscribers as specified in
    /// docs/features/runtime-control-plane.md.
    #[allow(dead_code)]
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

    /// Publish an adapter-produced event without changing its provenance or
    /// sequence. Protocol adapters use this after `control_plane::dispatch`
    /// has already wrapped an agent event for the originating session.
    pub fn publish_control_event(&self, event: RuntimeControlEvent) -> usize {
        if self.control_sender.receiver_count() == 0 {
            return 0;
        }
        self.control_sender.send(event).unwrap_or(0)
    }

    /// Publish an in-process control event using the bus-owned ordering domain.
    ///
    /// Control-plane dispatch assigns request-local sequence numbers. Local
    /// runtime consumers need one monotonically increasing stream across
    /// requests, while external protocol adapters must keep using
    /// [`Self::publish_control_event`] to preserve their original identity.
    pub(crate) fn publish_resequenced_control_event(
        &self,
        mut event: RuntimeControlEvent,
    ) -> usize {
        if self.control_sender.receiver_count() == 0 {
            return 0;
        }
        let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        event.event_id = format!("ctl-{sequence:016x}");
        event.sequence = sequence;
        self.project_tool_identity(&mut event);
        self.control_sender.send(event).unwrap_or(0)
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
        assert_eq!(
            bus.send_with_provenance(
                AgentEvent::AssistantDelta("hello".to_string()),
                RuntimeProvenance::runtime(None),
            ),
            2
        );

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

        assert_eq!(
            bus.send_with_provenance(
                AgentEvent::Status("ignored".to_string()),
                RuntimeProvenance::runtime(None),
            ),
            0
        );

        let mut raw = bus.subscribe();
        assert_eq!(
            bus.send_with_provenance(
                AgentEvent::Status("raw only".to_string()),
                RuntimeProvenance::runtime(None),
            ),
            1
        );
        assert!(matches!(
            raw.try_recv().expect("raw event"),
            AgentEvent::Status(message) if message == "raw only"
        ));
        drop(raw);

        let mut control = bus.subscribe_control();
        assert_eq!(
            bus.send_with_provenance(
                AgentEvent::Status("first control".to_string()),
                RuntimeProvenance::runtime(None),
            ),
            1
        );

        let event = control.try_recv().expect("control event");
        assert_eq!(event.sequence, 1);
        assert_eq!(event.event_id, "evt-0000000000000001");
    }

    #[test]
    fn adapter_events_keep_their_acp_provenance() {
        let bus = RuntimeEventBus::new(8);
        let mut control = bus.subscribe_control();
        let event = RuntimeControlEvent {
            event_id: "acp-1".to_string(),
            provenance: RuntimeProvenance::protocol(
                crate::runtime_control::RuntimeControllerKind::Acp,
                "acp",
                Some("session-1".to_string()),
                None,
            ),
            sequence: 7,
            event: RuntimeEvent::Session(crate::runtime_control::SessionEvent::TurnCancelled),
        };

        assert_eq!(bus.publish_control_event(event), 1);
        let received = control.try_recv().expect("control event");
        assert_eq!(received.event_id, "acp-1");
        assert_eq!(received.sequence, 7);
        assert_eq!(received.provenance.session_id.as_deref(), Some("session-1"));
    }

    #[test]
    fn in_process_events_are_resequenced_across_dispatch_requests() {
        let bus = RuntimeEventBus::new(8);
        let mut control = bus.subscribe_control();
        let provenance = RuntimeProvenance::local_tui("session-1");

        for event in [
            RuntimeEvent::Session(crate::runtime_control::SessionEvent::TurnStarted),
            RuntimeEvent::Session(crate::runtime_control::SessionEvent::TurnFinished {
                reason: Some("turn complete".into()),
            }),
        ] {
            let local_event = RuntimeControlEvent {
                event_id: "evt-dispatch-1".into(),
                provenance: provenance.clone(),
                sequence: 1,
                event,
            };
            assert_eq!(bus.publish_resequenced_control_event(local_event), 1);
        }

        let started = control.try_recv().expect("turn started");
        let finished = control.try_recv().expect("turn finished");
        assert_eq!((started.sequence, finished.sequence), (1, 2));
        assert_eq!(started.event_id, "ctl-0000000000000001");
        assert_eq!(finished.event_id, "ctl-0000000000000002");
        assert_eq!(started.provenance, provenance);
        assert_eq!(finished.provenance, provenance);
    }

    #[test]
    fn raw_lifecycle_events_do_not_duplicate_control_boundaries() {
        let bus = RuntimeEventBus::new(8);
        let mut raw = bus.subscribe();
        let mut control = bus.subscribe_control();

        assert_eq!(bus.publish_raw(AgentEvent::AgentStart), 1);
        assert!(matches!(
            raw.try_recv().expect("raw lifecycle event"),
            AgentEvent::AgentStart
        ));
        assert!(control.try_recv().is_err());
    }

    #[test]
    fn runtime_projects_one_tool_identity_across_use_progress_and_result() {
        let bus = RuntimeEventBus::new(8);
        let mut control = bus.subscribe_control();
        let provenance = RuntimeProvenance::local_tui("session-1");

        bus.send_with_provenance(
            AgentEvent::ToolUse {
                name: "bash".into(),
                input: json!({"command": "pwd"}),
            },
            provenance.clone(),
        );
        bus.send_with_provenance(
            AgentEvent::ToolProgress {
                name: "bash".into(),
                stream: rara_tools::tool::ToolOutputStream::Stdout,
                chunk: "/workspace\n".into(),
            },
            provenance.clone(),
        );
        bus.send_with_provenance(
            AgentEvent::ToolResult {
                name: "bash".into(),
                content: "done".into(),
                is_error: false,
            },
            provenance,
        );

        let use_id = match control.try_recv().expect("tool use").event {
            RuntimeEvent::Tool(crate::runtime_control::ToolEvent::Use {
                call_id: Some(call_id),
                ..
            }) => call_id,
            event => panic!("expected identified tool use, got {event:?}"),
        };
        let progress_id = match control.try_recv().expect("tool progress").event {
            RuntimeEvent::Tool(crate::runtime_control::ToolEvent::Progress {
                call_id: Some(call_id),
                ..
            }) => call_id,
            event => panic!("expected identified tool progress, got {event:?}"),
        };
        let result_id = match control.try_recv().expect("tool result").event {
            RuntimeEvent::Tool(crate::runtime_control::ToolEvent::Result {
                call_id: Some(call_id),
                ..
            }) => call_id,
            event => panic!("expected identified tool result, got {event:?}"),
        };

        assert_eq!(use_id, progress_id);
        assert_eq!(use_id, result_id);
    }

    #[test]
    fn runtime_keeps_tool_identities_isolated_between_sessions() {
        let bus = RuntimeEventBus::new(8);
        let mut control = bus.subscribe_control();

        for session in ["session-a", "session-b"] {
            bus.send_with_provenance(
                AgentEvent::ToolUse {
                    name: "bash".into(),
                    input: json!({"command": session}),
                },
                RuntimeProvenance::local_tui(session),
            );
        }
        let first_id = match control.try_recv().expect("first tool use").event {
            RuntimeEvent::Tool(crate::runtime_control::ToolEvent::Use {
                call_id: Some(call_id),
                ..
            }) => call_id,
            event => panic!("expected identified tool use, got {event:?}"),
        };
        let second_id = match control.try_recv().expect("second tool use").event {
            RuntimeEvent::Tool(crate::runtime_control::ToolEvent::Use {
                call_id: Some(call_id),
                ..
            }) => call_id,
            event => panic!("expected identified tool use, got {event:?}"),
        };
        assert_ne!(first_id, second_id);

        bus.send_with_provenance(
            AgentEvent::ToolResult {
                name: "bash".into(),
                content: "done".into(),
                is_error: false,
            },
            RuntimeProvenance::local_tui("session-a"),
        );
        let result_id = match control.try_recv().expect("session result").event {
            RuntimeEvent::Tool(crate::runtime_control::ToolEvent::Result {
                call_id: Some(call_id),
                ..
            }) => call_id,
            event => panic!("expected identified tool result, got {event:?}"),
        };
        assert_eq!(result_id, first_id);
    }
}
