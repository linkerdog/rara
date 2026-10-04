//! Per-instance counters at production work boundaries, compiled only in tests.

use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptWork {
    pub cloned_rows: usize,
    pub wrapped_lines: usize,
    pub text_rows: usize,
    pub hashed_rows: usize,
}

pub(crate) enum WorkKind {
    Clone,
    Wrap,
    Text,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WorkMeter(Rc<Cell<TranscriptWork>>);

impl WorkMeter {
    pub(crate) fn record(&self, kind: WorkKind, rows: usize) {
        let mut work = self.0.get();
        match kind {
            WorkKind::Clone => work.cloned_rows += rows,
            WorkKind::Wrap => work.wrapped_lines += rows,
            WorkKind::Text => work.text_rows += rows,
        }
        self.0.set(work);
    }

    pub(crate) fn get(&self) -> TranscriptWork {
        self.0.get()
    }
}
