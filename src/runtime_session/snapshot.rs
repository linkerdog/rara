pub use rara_runtime::RuntimeSessionPhase;

/// Point-in-time session state paired with the ordered event cursor.
pub type RuntimeSessionSnapshot = rara_runtime::SessionSnapshot<super::RuntimePendingInput>;
