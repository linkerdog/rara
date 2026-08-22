use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{broadcast, watch};

use super::{RuntimeSessionError, RuntimeSessionPhase, RuntimeSessionSnapshot};
use crate::runtime_control::RuntimeControlEvent;
use crate::runtime_event_bus::{RuntimeEventBus, RuntimeReplayGap};

/// Race-free snapshot and ordered event stream for one session observer.
pub struct RuntimeSessionSubscription {
    pub snapshot: RuntimeSessionSnapshot,
    pub events: RuntimeEventStream,
}

/// Ordered control-event stream backed by the session replay window.
pub struct RuntimeEventStream {
    event_bus: Arc<RuntimeEventBus>,
    live: broadcast::Receiver<RuntimeControlEvent>,
    lifecycle: watch::Receiver<RuntimeSessionSnapshot>,
    replay: VecDeque<RuntimeControlEvent>,
    cursor: u64,
}

impl RuntimeEventStream {
    pub(crate) fn new(
        event_bus: Arc<RuntimeEventBus>,
        live: broadcast::Receiver<RuntimeControlEvent>,
        lifecycle: watch::Receiver<RuntimeSessionSnapshot>,
        replay: Vec<RuntimeControlEvent>,
        cursor: u64,
    ) -> Self {
        Self {
            event_bus,
            live,
            lifecycle,
            replay: replay.into(),
            cursor,
        }
    }

    /// Return the last sequence delivered to this observer.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Receive the next event, recovering broadcast lag from the replay window.
    pub async fn recv(&mut self) -> Result<RuntimeControlEvent, RuntimeSessionError> {
        loop {
            if let Some(event) = self.replay.pop_front() {
                if event.sequence > self.cursor {
                    self.cursor = event.sequence;
                    return Ok(event);
                }
                continue;
            }

            match self.live.try_recv() {
                Ok(event) if event.sequence > self.cursor => {
                    self.cursor = event.sequence;
                    return Ok(event);
                }
                Ok(_) => continue,
                Err(broadcast::error::TryRecvError::Empty) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    self.replay = self
                        .event_bus
                        .replay_after(self.cursor)
                        .map_err(replay_gap_error)?
                        .into();
                    continue;
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    return Err(RuntimeSessionError::ActorStopped);
                }
            }

            if matches!(self.lifecycle.borrow().phase, RuntimeSessionPhase::Closed) {
                return Err(RuntimeSessionError::Closed);
            }

            let received = tokio::select! {
                biased;
                event = self.live.recv() => Some(event),
                changed = self.lifecycle.changed() => {
                    match changed {
                        Ok(()) => None,
                        Err(_) => return Err(RuntimeSessionError::ActorStopped),
                    }
                }
            };
            let Some(received) = received else {
                continue;
            };
            match received {
                Ok(event) if event.sequence > self.cursor => {
                    self.cursor = event.sequence;
                    return Ok(event);
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    self.replay = self
                        .event_bus
                        .replay_after(self.cursor)
                        .map_err(replay_gap_error)?
                        .into();
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(RuntimeSessionError::ActorStopped);
                }
            }
        }
    }
}

pub(crate) fn replay_gap_error(gap: RuntimeReplayGap) -> RuntimeSessionError {
    RuntimeSessionError::ResyncRequired {
        requested: gap.requested,
        oldest_available: gap.oldest_available,
        latest: gap.latest,
    }
}
