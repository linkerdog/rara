use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use super::RuntimeProjectionEvent;
use crate::runtime_control::RuntimeControlEvent;
use crate::runtime_event_bus::{RuntimeEventBus, RuntimeReplayGap};

/// Keep transport ordering ahead of the controller's session/turn filtering.
pub(super) struct ReplayingEventStream {
    bus: Arc<RuntimeEventBus>,
    live: BroadcastStream<RuntimeControlEvent>,
    replay: VecDeque<RuntimeControlEvent>,
    pending_live: Option<RuntimeControlEvent>,
    cursor: u64,
    replay_required: bool,
}

impl ReplayingEventStream {
    pub(super) fn new(bus: Arc<RuntimeEventBus>, cursor: u64) -> Self {
        // Capture first, then subscribe and replay: publication between these
        // operations is retained even when it did not reach the live receiver.
        let live = BroadcastStream::new(bus.subscribe_control());
        Self {
            bus,
            live,
            replay: VecDeque::new(),
            pending_live: None,
            cursor,
            replay_required: false,
        }
    }
}

impl Stream for ReplayingEventStream {
    type Item = RuntimeProjectionEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let mut remaining = 64;
        loop {
            // Continuous publication must not monopolize the UI while stale
            // live copies are being discarded or a receiver keeps lagging.
            if remaining == 0 {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            remaining -= 1;
            if let Some(event) = this.replay.pop_front() {
                if event.sequence <= this.cursor {
                    continue;
                }
                this.cursor = event.sequence;
                return Poll::Ready(Some(RuntimeProjectionEvent::Runtime(Box::new(event))));
            }
            if this.replay_required {
                this.replay_required = false;
                match this.bus.replay_after(this.cursor) {
                    Ok(events) => this.replay = events.into(),
                    Err(mut gap) => {
                        if let Some(event) = &this.pending_live {
                            gap.oldest_available = gap.oldest_available.min(event.sequence);
                        }
                        if gap.oldest_available > this.cursor.saturating_add(1) {
                            this.cursor = gap.oldest_available - 1;
                            this.replay_required = this
                                .pending_live
                                .as_ref()
                                .is_none_or(|event| event.sequence > gap.oldest_available);
                        }
                        return Poll::Ready(Some(RuntimeProjectionEvent::ResyncRequired(gap)));
                    }
                }
                continue;
            }
            if let Some(event) = this.pending_live.take() {
                if event.sequence <= this.cursor {
                    continue;
                }
                if event.sequence > this.cursor.saturating_add(1) {
                    let gap = RuntimeReplayGap {
                        requested: this.cursor,
                        oldest_available: event.sequence,
                        latest: event.sequence,
                    };
                    this.cursor = event.sequence - 1;
                    this.pending_live = Some(event);
                    return Poll::Ready(Some(RuntimeProjectionEvent::ResyncRequired(gap)));
                }
                this.cursor = event.sequence;
                return Poll::Ready(Some(RuntimeProjectionEvent::Runtime(Box::new(event))));
            }
            match this.live.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    if event.sequence == 0 {
                        return Poll::Ready(Some(RuntimeProjectionEvent::Runtime(Box::new(event))));
                    }
                    if event.sequence <= this.cursor {
                        continue;
                    }
                    if event.sequence > this.cursor.saturating_add(1) {
                        this.pending_live = Some(event);
                        this.replay_required = true;
                        continue;
                    }
                    this.cursor = event.sequence;
                    return Poll::Ready(Some(RuntimeProjectionEvent::Runtime(Box::new(event))));
                }
                // Read the oldest live event before replay: Tokio can retain
                // more events than the log when it rounds channel capacity up.
                Poll::Ready(Some(Err(_))) => {}
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => {
                    if this.bus.current_sequence() > this.cursor {
                        this.replay_required = true;
                    } else {
                        return Poll::Pending;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;

    use super::*;
    use crate::agent::AgentEvent;
    use crate::runtime_control::RuntimeProvenance;

    #[tokio::test]
    async fn a_sequence_gap_replays_even_without_a_lag_notification() {
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut stream = ReplayingEventStream::new(bus.clone(), 0);
        assert!(stream.next().now_or_never().is_none());
        for n in 1..=2 {
            bus.send_with_provenance(
                AgentEvent::Status(n.to_string()),
                RuntimeProvenance::local_tui("test"),
            );
        }
        // Simulate a transport reconnect that omitted two retained events.
        stream.live = BroadcastStream::new(bus.subscribe_control());
        bus.send_with_provenance(
            AgentEvent::Status("three".into()),
            RuntimeProvenance::local_tui("test"),
        );
        for sequence in 1..=3 {
            assert!(
                matches!(stream.next().await, Some(RuntimeProjectionEvent::Runtime(event)) if event.sequence == sequence)
            );
        }
        assert!(stream.next().now_or_never().is_none());
    }
}
