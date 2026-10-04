use super::RuntimeSessionError;

/// Retained cleanup evidence, independent of channel or event stream closure.
#[derive(Clone, Copy)]
pub(super) enum ShutdownOutcome {
    Complete,
    Failed,
}

impl ShutdownOutcome {
    pub(super) fn result(self) -> Result<(), RuntimeSessionError> {
        match self {
            Self::Complete => Ok(()),
            Self::Failed => Err(RuntimeSessionError::ShutdownFailed),
        }
    }
}
