use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::broadcast;

/// Exposes the session-assigned ordering identity without changing the payload.
pub trait SequencedEvent: Clone + Send + 'static {
    fn sequence(&self) -> u64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayGap {
    pub requested: u64,
    pub oldest_available: u64,
    pub latest: u64,
}

impl From<ReplayGap> for crate::RuntimeSessionError {
    fn from(gap: ReplayGap) -> Self {
        Self::ResyncRequired {
            requested: gap.requested,
            oldest_available: gap.oldest_available,
            latest: gap.latest,
        }
    }
}

#[derive(Debug)]
struct Publication<E> {
    sequence: u64,
    replay: VecDeque<E>,
}

/// One ordered broadcast and bounded replay domain for a session.
#[derive(Clone, Debug)]
pub struct EventLog<E> {
    sender: broadcast::Sender<E>,
    publication: Arc<Mutex<Publication<E>>>,
    capacity: usize,
}

impl<E: SequencedEvent> EventLog<E> {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, _) = broadcast::channel(capacity);
        Self::with_sender(sender, capacity)
    }

    pub fn with_sender(sender: broadcast::Sender<E>, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            sender,
            publication: Arc::new(Mutex::new(Publication {
                sequence: 0,
                replay: VecDeque::with_capacity(capacity),
            })),
            capacity,
        }
    }

    /// Allocate identity, retain, and broadcast atomically. Neither callback may
    /// re-enter this log; the receipt callback runs after retention but before delivery.
    pub fn publish(&self, build: impl FnOnce(u64) -> E, retain_receipt: impl FnOnce(&E)) -> usize {
        let mut publication = self.lock_publication();
        publication.sequence += 1;
        let event = build(publication.sequence);
        publication.replay.push_back(event.clone());
        while publication.replay.len() > self.capacity {
            publication.replay.pop_front();
        }
        retain_receipt(&event);
        // No subscribers is normal; the event remains available for replay.
        self.sender.send(event).unwrap_or(0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<E> {
        self.sender.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }

    pub fn current_sequence(&self) -> u64 {
        self.lock_publication().sequence
    }

    pub fn replay_after(&self, sequence: u64) -> Result<Vec<E>, ReplayGap> {
        let publication = self.lock_publication();
        let latest = publication.sequence;
        if sequence == latest {
            return Ok(Vec::new());
        }
        let oldest_available = publication
            .replay
            .front()
            .map(SequencedEvent::sequence)
            .unwrap_or_else(|| latest.saturating_add(1));
        if sequence > latest || sequence.saturating_add(1) < oldest_available {
            return Err(ReplayGap {
                requested: sequence,
                oldest_available,
                latest,
            });
        }
        Ok(publication
            .replay
            .iter()
            .filter(|event| event.sequence() > sequence)
            .cloned()
            .collect())
    }

    fn lock_publication(&self) -> MutexGuard<'_, Publication<E>> {
        self.publication.lock().unwrap_or_else(|poisoned| {
            log::warn!("runtime event publication lock was poisoned; recovering");
            poisoned.into_inner()
        })
    }
}
