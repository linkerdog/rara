use super::{RuntimePendingInput, RuntimeSessionError, RuntimeSessionSnapshot};
use crate::runtime_control::RuntimeControlEvent;
use crate::runtime_event_bus::RuntimeReplayGap;

/// Race-free snapshot and ordered event stream for one session observer.
pub struct RuntimeSessionSubscription {
    pub snapshot: RuntimeSessionSnapshot,
    pub events: RuntimeEventStream,
}

pub type RuntimeEventStream = rara_runtime::EventStream<RuntimeControlEvent, RuntimePendingInput>;

pub(crate) fn replay_gap_error(gap: RuntimeReplayGap) -> RuntimeSessionError {
    gap.into()
}
