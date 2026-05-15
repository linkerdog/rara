use std::collections::VecDeque;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const DEFAULT_LATENCY_CAPACITY: usize = 128;

static GLOBAL_MEMORY_OBSERVABILITY: OnceLock<Arc<MemoryObservability>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOperation {
    Read,
    Write,
    Query,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyPercentiles {
    pub sample_count: usize,
    pub p80_ms: Option<u64>,
    pub p99_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryLatencySnapshot {
    pub read: LatencyPercentiles,
    pub write: LatencyPercentiles,
    pub query: LatencyPercentiles,
}

impl MemoryLatencySnapshot {
    pub fn is_empty(self) -> bool {
        self.read.sample_count == 0 && self.write.sample_count == 0 && self.query.sample_count == 0
    }
}

#[derive(Debug)]
pub struct MemoryObservability {
    inner: RwLock<MemoryLatencyState>,
}

impl Default for MemoryObservability {
    fn default() -> Self {
        Self::new(DEFAULT_LATENCY_CAPACITY)
    }
}

impl MemoryObservability {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            inner: RwLock::new(MemoryLatencyState::new(capacity)),
        }
    }

    pub fn record_latency(&self, operation: MemoryOperation, duration: Duration) {
        let Ok(mut inner) = self.inner.try_write() else {
            return;
        };
        inner.record(operation, duration);
    }

    pub fn start_timer(&self, operation: MemoryOperation) -> MemoryLatencyTimer<'_> {
        MemoryLatencyTimer {
            observability: self,
            operation,
            start: Instant::now(),
        }
    }

    pub fn snapshot(&self) -> MemoryLatencySnapshot {
        let Ok(inner) = self.inner.try_read() else {
            return MemoryLatencySnapshot::default();
        };
        inner.snapshot()
    }
}

#[derive(Debug)]
pub struct MemoryLatencyTimer<'a> {
    observability: &'a MemoryObservability,
    operation: MemoryOperation,
    start: Instant,
}

impl Drop for MemoryLatencyTimer<'_> {
    fn drop(&mut self) {
        self.observability
            .record_latency(self.operation, self.start.elapsed());
    }
}

#[derive(Debug)]
struct MemoryLatencyState {
    read: LatencyWindow,
    write: LatencyWindow,
    query: LatencyWindow,
}

impl MemoryLatencyState {
    fn new(capacity: usize) -> Self {
        Self {
            read: LatencyWindow::new(capacity),
            write: LatencyWindow::new(capacity),
            query: LatencyWindow::new(capacity),
        }
    }

    fn record(&mut self, operation: MemoryOperation, duration: Duration) {
        match operation {
            MemoryOperation::Read => self.read.record(duration),
            MemoryOperation::Write => self.write.record(duration),
            MemoryOperation::Query => self.query.record(duration),
        }
    }

    fn snapshot(&self) -> MemoryLatencySnapshot {
        MemoryLatencySnapshot {
            read: self.read.percentiles(),
            write: self.write.percentiles(),
            query: self.query.percentiles(),
        }
    }
}

#[derive(Debug)]
struct LatencyWindow {
    capacity: usize,
    samples_ms: VecDeque<u64>,
}

impl LatencyWindow {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            samples_ms: VecDeque::with_capacity(capacity),
        }
    }

    fn record(&mut self, duration: Duration) {
        if self.samples_ms.len() == self.capacity {
            self.samples_ms.pop_front();
        }
        self.samples_ms.push_back(duration_to_ms(duration));
    }

    fn percentiles(&self) -> LatencyPercentiles {
        if self.samples_ms.is_empty() {
            return LatencyPercentiles::default();
        }
        let mut values = self.samples_ms.iter().copied().collect::<Vec<_>>();
        values.sort_unstable();
        LatencyPercentiles {
            sample_count: values.len(),
            p80_ms: Some(percentile_nearest_rank(&values, 80)),
            p99_ms: Some(percentile_nearest_rank(&values, 99)),
        }
    }
}

fn duration_to_ms(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    millis.min(u128::from(u64::MAX)) as u64
}

fn percentile_nearest_rank(sorted_values: &[u64], percentile: usize) -> u64 {
    debug_assert!(!sorted_values.is_empty());
    let rank = sorted_values.len().saturating_mul(percentile).div_ceil(100);
    let index = rank.saturating_sub(1).min(sorted_values.len() - 1);
    sorted_values[index]
}

pub fn global_memory_observability() -> Arc<MemoryObservability> {
    GLOBAL_MEMORY_OBSERVABILITY
        .get_or_init(|| Arc::new(MemoryObservability::default()))
        .clone()
}

pub fn memory_latency_snapshot() -> MemoryLatencySnapshot {
    global_memory_observability().snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_window_caps_samples_and_reports_percentiles() {
        let observability = MemoryObservability::new(5);
        for value in 1..=10 {
            observability.record_latency(MemoryOperation::Query, Duration::from_millis(value));
        }

        let snapshot = observability.snapshot();

        assert_eq!(snapshot.query.sample_count, 5);
        assert_eq!(snapshot.query.p80_ms, Some(9));
        assert_eq!(snapshot.query.p99_ms, Some(10));
        assert_eq!(snapshot.read.sample_count, 0);
        assert_eq!(snapshot.write.sample_count, 0);
    }

    #[test]
    fn timer_records_elapsed_latency_on_drop() {
        let observability = Arc::new(MemoryObservability::new(4));
        {
            let _timer = observability.start_timer(MemoryOperation::Read);
        }

        let snapshot = observability.snapshot();

        assert_eq!(snapshot.read.sample_count, 1);
        assert!(snapshot.read.p80_ms.is_some());
        assert!(snapshot.read.p99_ms.is_some());
    }
}
