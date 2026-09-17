//! Public, session-scoped runtime ownership.

mod actor;
mod builder;
mod command;
mod error;
mod handle;
mod host;
mod ids;
mod input;
mod profile;
mod shutdown;
mod snapshot;
mod subscription;
mod turn;

pub use builder::RuntimeSessionBuilder;
pub use error::RuntimeSessionError;
pub use handle::RuntimeSession;
pub use host::RuntimeHost;
pub use ids::{RuntimeSessionId, RuntimeTurnId};
pub use input::{RuntimeInput, RuntimeInputAnswer, RuntimePendingInput, RuntimePendingInputKind};
pub use profile::RuntimeSessionProfile;
pub use snapshot::{RuntimeSessionPhase, RuntimeSessionSnapshot};
pub use subscription::{RuntimeEventStream, RuntimeSessionSubscription};
pub use turn::{RuntimeTurn, RuntimeTurnOutcome};

#[cfg(test)]
mod source_tests;

#[cfg(test)]
mod input_tests;
