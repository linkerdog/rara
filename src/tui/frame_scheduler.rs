//! Coalesces repaint requests without delaying ordered presentation events.

use std::future::pending;

use tokio::time::{Duration, Instant};

pub(super) const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

#[derive(Debug, Default)]
pub(super) struct FrameScheduler {
    deadline: Option<Instant>,
    last_drawn_at: Option<Instant>,
}

impl FrameScheduler {
    pub(super) fn request(&mut self, requested_at: Instant) {
        let earliest = self.last_drawn_at.map_or(requested_at, |last| {
            requested_at.max(last + MIN_FRAME_INTERVAL)
        });
        self.deadline = Some(
            self.deadline
                .map_or(earliest, |pending| pending.min(earliest)),
        );
    }

    pub(super) fn is_due(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| deadline <= now)
    }

    pub(super) fn mark_drawn(&mut self, completed_at: Instant) {
        self.deadline = None;
        self.last_drawn_at = Some(completed_at);
    }

    pub(super) async fn wait(&self) {
        match self.deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => pending::<()>().await,
        }
    }
}

#[cfg(test)]
#[path = "frame_scheduler_tests.rs"]
mod tests;
