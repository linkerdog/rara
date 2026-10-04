use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{broadcast, watch};

use crate::{EventLog, RuntimeSessionError, RuntimeSessionPhase, SequencedEvent, SessionSnapshot};

/// Ordered control-event stream backed by the session replay window.
pub struct EventStream<E, P> {
    event_bus: Arc<EventLog<E>>,
    live: broadcast::Receiver<E>,
    lifecycle: watch::Receiver<SessionSnapshot<P>>,
    replay: VecDeque<E>,
    cursor: u64,
}

impl<E: SequencedEvent, P: Clone> EventStream<E, P> {
    pub fn new(
        event_bus: Arc<EventLog<E>>,
        live: broadcast::Receiver<E>,
        lifecycle: watch::Receiver<SessionSnapshot<P>>,
        replay: Vec<E>,
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
    pub async fn recv(&mut self) -> Result<E, RuntimeSessionError> {
        loop {
            if let Some(event) = self.replay.pop_front() {
                if event.sequence() > self.cursor {
                    self.cursor = event.sequence();
                    return Ok(event);
                }
                continue;
            }

            match self.live.try_recv() {
                Ok(event) if event.sequence() > self.cursor => {
                    self.cursor = event.sequence();
                    return Ok(event);
                }
                Ok(_) => continue,
                Err(broadcast::error::TryRecvError::Empty) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    self.replay = self
                        .event_bus
                        .replay_after(self.cursor)
                        .map_err(RuntimeSessionError::from)?
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
                Ok(event) if event.sequence() > self.cursor => {
                    self.cursor = event.sequence();
                    return Ok(event);
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    self.replay = self
                        .event_bus
                        .replay_after(self.cursor)
                        .map_err(RuntimeSessionError::from)?
                        .into();
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(RuntimeSessionError::ActorStopped);
                }
            }
        }
    }
}
