use std::collections::VecDeque;
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
    publication: Arc<Mutex<()>>,
    replay_capacity: usize,
    replay: Arc<Mutex<VecDeque<RuntimeControlEvent>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeReplayGap {
    pub(crate) requested: u64,
    pub(crate) oldest_available: u64,
    pub(crate) latest: u64,
}

impl RuntimeEventBus {
    /// Create a new bus with a fixed ring-buffer capacity.  When the buffer
    /// is full the oldest event is dropped for the slowest subscriber.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (raw_sender, _) = broadcast::channel(capacity);
        let (control_sender, _) = broadcast::channel(capacity);
        Self {
            raw_sender,
            control_sender,
            next_sequence: Arc::new(AtomicU64::new(0)),
            publication: Arc::new(Mutex::new(())),
            replay_capacity: capacity,
            replay: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
        }
    }

    /// Push an event with explicit provenance for protocol-ready subscribers.
    pub fn send_with_provenance(&self, event: AgentEvent, provenance: RuntimeProvenance) -> usize {
        self.send_with_turn(event, provenance, None)
    }

    /// Push an event in the ordered session stream with optional turn identity.
    pub(crate) fn send_with_turn(
        &self,
        event: AgentEvent,
        provenance: RuntimeProvenance,
        turn_id: Option<&str>,
    ) -> usize {
        let _publication = self.lock_publication();
        let raw_receivers = self.raw_sender.receiver_count();
        let control_receivers = self.control_sender.receiver_count();
        let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let event_id = format!("evt-{sequence:016x}");
        let mut control_event = wrap_agent_event(event_id, sequence, provenance, event.clone());
        control_event.turn_id = turn_id.map(str::to_string);
        self.record_control_event(control_event.clone());
        let raw_count = if raw_receivers > 0 {
            self.raw_sender.send(event).unwrap_or(0)
        } else {
            0
        };
        let control_count = if control_receivers > 0 {
            self.control_sender.send(control_event).unwrap_or(0)
        } else {
            0
        };
        raw_count + control_count
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
    pub fn subscribe_control(&self) -> broadcast::Receiver<RuntimeControlEvent> {
        self.control_sender.subscribe()
    }

    /// Return the number of active subscribers.
    pub fn receiver_count(&self) -> usize {
        self.raw_sender.receiver_count() + self.control_sender.receiver_count()
    }

    /// Return the latest sequence assigned in this bus ordering domain.
    pub fn current_sequence(&self) -> u64 {
        let _publication = self.lock_publication();
        self.next_sequence.load(Ordering::SeqCst)
    }

    pub(crate) fn replay_after(
        &self,
        sequence: u64,
    ) -> Result<Vec<RuntimeControlEvent>, RuntimeReplayGap> {
        let _publication = self.lock_publication();
        let latest = self.next_sequence.load(Ordering::SeqCst);
        if sequence >= latest {
            return Ok(Vec::new());
        }
        let replay = match self.replay.lock() {
            Ok(replay) => replay,
            Err(poisoned) => {
                log::warn!("runtime event replay lock was poisoned; recovering");
                poisoned.into_inner()
            }
        };
        let oldest_available = replay
            .front()
            .map(|event| event.sequence)
            .unwrap_or(latest + 1);
        if sequence.saturating_add(1) < oldest_available {
            return Err(RuntimeReplayGap {
                requested: sequence,
                oldest_available,
                latest,
            });
        }
        Ok(replay
            .iter()
            .filter(|event| event.sequence > sequence)
            .cloned()
            .collect())
    }

    /// Publish a structured `RuntimeEvent` on the control bus without wrapping
    /// an `AgentEvent`. Used for protocol-native events (MCP, hooks, etc.) that
    /// originate from the runtime itself rather than from agent execution.
    pub fn publish_control(&self, event: RuntimeEvent) -> usize {
        self.publish_control_with_turn(event, RuntimeProvenance::runtime(None), None)
    }

    pub(crate) fn publish_control_with_turn(
        &self,
        event: RuntimeEvent,
        provenance: RuntimeProvenance,
        turn_id: Option<&str>,
    ) -> usize {
        let _publication = self.lock_publication();
        let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let event_id = format!("ctl-{sequence:016x}");
        let control_event = RuntimeControlEvent {
            event_id,
            provenance,
            turn_id: turn_id.map(str::to_string),
            sequence,
            event,
        };
        self.record_control_event(control_event.clone());
        if self.control_sender.receiver_count() > 0 {
            self.control_sender.send(control_event).unwrap_or(0)
        } else {
            0
        }
    }

    /// Publish an adapter-produced event without changing its provenance or
    /// sequence. Protocol adapters use this after `control_plane::dispatch`
    /// has already wrapped an agent event for the originating session.
    #[cfg(test)]
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
    /// requests.
    pub(crate) fn publish_resequenced_control_event(
        &self,
        mut event: RuntimeControlEvent,
    ) -> usize {
        let _publication = self.lock_publication();
        let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        event.event_id = format!("ctl-{sequence:016x}");
        event.sequence = sequence;
        self.record_control_event(event.clone());
        if self.control_sender.receiver_count() > 0 {
            self.control_sender.send(event).unwrap_or(0)
        } else {
            0
        }
    }

    fn record_control_event(&self, event: RuntimeControlEvent) {
        let mut replay = match self.replay.lock() {
            Ok(replay) => replay,
            Err(poisoned) => {
                log::warn!("runtime event replay lock was poisoned; recovering");
                poisoned.into_inner()
            }
        };
        replay.push_back(event);
        while replay.len() > self.replay_capacity {
            replay.pop_front();
        }
    }

    fn lock_publication(&self) -> std::sync::MutexGuard<'_, ()> {
        match self.publication.lock() {
            Ok(publication) => publication,
            Err(poisoned) => {
                log::warn!("runtime event publication lock was poisoned; recovering");
                poisoned.into_inner()
            }
        }
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
    fn sequence_advances_even_without_control_subscribers() {
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
        assert_eq!(event.sequence, 3);
        assert_eq!(event.event_id, "evt-0000000000000003");
        assert_eq!(bus.current_sequence(), 3);
    }

    #[test]
    fn replay_reports_a_gap_after_the_bounded_window_is_exhausted() {
        let bus = RuntimeEventBus::new(2);
        for message in ["one", "two", "three"] {
            bus.send_with_provenance(
                AgentEvent::Status(message.to_string()),
                RuntimeProvenance::runtime(Some("session-1".to_string())),
            );
        }

        assert_eq!(
            bus.replay_after(0),
            Err(RuntimeReplayGap {
                requested: 0,
                oldest_available: 2,
                latest: 3,
            })
        );
        let replay = bus.replay_after(1).expect("bounded replay");
        assert_eq!(
            replay
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
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
            turn_id: None,
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
                turn_id: None,
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
    fn runtime_preserves_provider_tool_identity_across_use_progress_and_result() {
        let bus = RuntimeEventBus::new(8);
        let mut control = bus.subscribe_control();
        let provenance = RuntimeProvenance::local_tui("session-1");

        bus.send_with_provenance(
            AgentEvent::ToolUse {
                call_id: "provider-call-1".into(),
                name: "bash".into(),
                input: json!({"command": "pwd"}),
            },
            provenance.clone(),
        );
        bus.send_with_provenance(
            AgentEvent::ToolProgress {
                call_id: "provider-call-1".into(),
                name: "bash".into(),
                stream: rara_tools::tool::ToolOutputStream::Stdout,
                chunk: "/workspace\n".into(),
            },
            provenance.clone(),
        );
        bus.send_with_provenance(
            AgentEvent::ToolResult {
                call_id: "provider-call-1".into(),
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
                    call_id: format!("{session}-call"),
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
                call_id: "session-a-call".into(),
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

    #[test]
    fn concurrent_publication_keeps_live_and_replay_sequence_order() {
        const PUBLISHERS: usize = 8;
        const EVENTS_PER_PUBLISHER: usize = 64;
        let event_count = PUBLISHERS * EVENTS_PER_PUBLISHER;
        let bus = Arc::new(RuntimeEventBus::new(event_count));
        let mut control = bus.subscribe_control();
        let barrier = Arc::new(std::sync::Barrier::new(PUBLISHERS));

        let publishers = (0..PUBLISHERS)
            .map(|publisher| {
                let bus = bus.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for event in 0..EVENTS_PER_PUBLISHER {
                        bus.send_with_provenance(
                            AgentEvent::Status(format!("{publisher}:{event}")),
                            RuntimeProvenance::local_tui(format!("session-{publisher}")),
                        );
                    }
                })
            })
            .collect::<Vec<_>>();
        for publisher in publishers {
            publisher.join().expect("event publisher");
        }

        let expected = (1..=event_count as u64).collect::<Vec<_>>();
        let live = (0..event_count)
            .map(|_| control.try_recv().expect("live event").sequence)
            .collect::<Vec<_>>();
        let replay = bus
            .replay_after(0)
            .expect("complete replay")
            .into_iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>();
        assert_eq!(live, expected);
        assert_eq!(replay, expected);
    }
}
