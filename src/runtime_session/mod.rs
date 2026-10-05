//! Public, session-scoped runtime ownership.

mod builder;
mod command;
mod driver;
mod error;
mod handle;
mod host;
mod ids;
mod input;
mod mcp_sources;
mod mcp_tool;
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
pub(crate) use mcp_sources::is_controlled_mcp_tool;
#[cfg(all(test, unix))]
pub(crate) use mcp_sources::tests::registration as mcp_source_registration_fixture;
pub use profile::RuntimeSessionProfile;
pub use snapshot::{RuntimeSessionPhase, RuntimeSessionSnapshot};
pub use subscription::{RuntimeEventStream, RuntimeSessionSubscription};
pub use turn::{RuntimeTurn, RuntimeTurnOutcome};

#[cfg(test)]
mod source_tests;

#[cfg(test)]
mod input_tests;

#[cfg(test)]
mod skill_tests;
