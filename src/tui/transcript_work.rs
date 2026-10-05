//! Per-instance counters at production work boundaries, compiled only in tests.

use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptWork {
    pub cloned_rows: usize,
    pub wrapped_lines: usize,
    pub text_rows: usize,
    pub hashed_rows: usize,
    // Byte work is currently instrumented at the streaming response boundary.
    pub cloned_bytes: usize,
    pub wrapped_bytes: usize,
}

pub(crate) enum WorkKind {
    Clone,
    Wrap,
    Text,
    CloneBytes,
    WrapBytes,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WorkMeter(Rc<Cell<TranscriptWork>>);

impl WorkMeter {
    pub(crate) fn record(&self, kind: WorkKind, amount: usize) {
        let mut work = self.0.get();
        match kind {
            WorkKind::Clone => work.cloned_rows += amount,
            WorkKind::Wrap => work.wrapped_lines += amount,
            WorkKind::Text => work.text_rows += amount,
            WorkKind::CloneBytes => work.cloned_bytes += amount,
            WorkKind::WrapBytes => work.wrapped_bytes += amount,
        }
        self.0.set(work);
    }

    pub(crate) fn get(&self) -> TranscriptWork {
        self.0.get()
    }
}
