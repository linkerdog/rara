use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::FutureExt;
use futures::StreamExt;

use super::*;
use crate::agent::AgentEvent;
use crate::runtime_control::RuntimeProvenance;
use crate::runtime_event_bus::RuntimeEventCapacity;

#[tokio::test]
async fn exhausted_broadcast_reports_loss_before_retained_events() {
    let bus = Arc::new(RuntimeEventBus::new(2));
    let (port, _commands) = InProcessRuntimeClientPort::new(
        bus.clone(),
        Arc::new(RwLock::new(RuntimeSnapshot::default())),
    );
    let mut stream = port.subscribe();
    for n in 1..=6 {
        bus.send_with_provenance(
            AgentEvent::Status(n.to_string()),
            RuntimeProvenance::local_tui("test"),
        );
    }
    let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        first,
        RuntimeProjectionEvent::ResyncRequired(RuntimeReplayGap {
            requested: 0,
            oldest_available: 5,
            latest: 6,
        })
    ));
    for sequence in 5..=6 {
        assert!(
            matches!(stream.next().await, Some(RuntimeProjectionEvent::Runtime(event)) if event.sequence == sequence)
        );
    }
    assert!(
        stream.next().now_or_never().is_none(),
        "no duplicate tail or loss notice"
    );
}

#[tokio::test]
async fn broadcast_overflow_replays_every_event_once_without_a_warning() {
    let bus = Arc::new(RuntimeEventBus::with_capacity(RuntimeEventCapacity {
        broadcast: 2,
        replay: 16,
    }));
    let (port, _commands) = InProcessRuntimeClientPort::new(
        bus.clone(),
        Arc::new(RwLock::new(RuntimeSnapshot::default())),
    );
    let mut stream = port.subscribe();
    assert!(stream.next().now_or_never().is_none());
    for sequence in 1..=12 {
        bus.send_with_provenance(
            AgentEvent::Status(sequence.to_string()),
            RuntimeProvenance::local_tui("test"),
        );
    }
    for sequence in 1..=12 {
        assert!(
            matches!(stream.next().await, Some(RuntimeProjectionEvent::Runtime(event)) if event.sequence == sequence)
        );
    }
    assert!(stream.next().now_or_never().is_none());
    bus.send_with_provenance(
        AgentEvent::Status("after cancellation".into()),
        RuntimeProvenance::local_tui("test"),
    );
    assert!(
        matches!(stream.next().await, Some(RuntimeProjectionEvent::Runtime(event)) if event.sequence == 13)
    );
}

#[tokio::test]
async fn replay_closes_the_cursor_subscription_race() {
    let bus = Arc::new(RuntimeEventBus::new(8));
    let (port, _commands) = InProcessRuntimeClientPort::new(
        bus.clone(),
        Arc::new(RwLock::new(RuntimeSnapshot::default())),
    );
    let cursor = port.current_sequence();
    bus.send_with_provenance(
        AgentEvent::Status("between capture and subscribe".into()),
        RuntimeProvenance::local_tui("test"),
    );
    let mut stream = port.subscribe_after(cursor);
    assert!(
        matches!(stream.next().await, Some(RuntimeProjectionEvent::Runtime(event)) if event.sequence == 1)
    );
}

#[tokio::test]
async fn live_buffer_records_are_kept_when_it_outlasts_the_replay_window() {
    // Tokio rounds three broadcast slots up to four; log retention stays three.
    let bus = Arc::new(RuntimeEventBus::new(3));
    let (port, _commands) = InProcessRuntimeClientPort::new(
        bus.clone(),
        Arc::new(RwLock::new(RuntimeSnapshot::default())),
    );
    let mut stream = port.subscribe();
    for sequence in 1..=4 {
        bus.send_with_provenance(
            AgentEvent::Status(sequence.to_string()),
            RuntimeProvenance::runtime(None),
        );
    }
    for sequence in 1..=4 {
        assert!(
            matches!(stream.next().await, Some(RuntimeProjectionEvent::Runtime(event)) if event.sequence == sequence)
        );
    }
    for sequence in 5..=9 {
        bus.send_with_provenance(
            AgentEvent::Status(sequence.to_string()),
            RuntimeProvenance::runtime(None),
        );
    }
    assert!(
        matches!(stream.next().await, Some(RuntimeProjectionEvent::ResyncRequired(gap)) if gap.requested == 4 && gap.oldest_available == 6)
    );
    for sequence in 6..=9 {
        assert!(
            matches!(stream.next().await, Some(RuntimeProjectionEvent::Runtime(event)) if event.sequence == sequence)
        );
    }
}
