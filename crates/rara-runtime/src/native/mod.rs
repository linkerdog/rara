//! Session ownership and host-controlled execution over the shared agent loop.

mod actor;
mod command;
mod driver;
mod error;
mod event_log;
mod event_stream;
mod events;
mod handle;
mod host_driver;
mod host_turn;
mod ids;
mod session;
mod shutdown;
mod snapshot;
mod turn;

pub use driver::{
    CompletedTurn, InputDiscardReason, PendingDisposition, PendingInteraction, SessionDriver,
    SessionLifecycle, SessionTurn, TurnContext, TurnFinishReason, TurnStopKind,
};
pub use error::RuntimeSessionError;
pub use event_log::{EventLog, ReplayGap, SequencedEvent};
pub use event_stream::EventStream;
pub use events::{AssistantEvent, RuntimeControlEvent, RuntimeEvent, SessionEvent, ToolEvent};
pub use handle::SessionHandle;
pub use host_driver::NoPendingInput;
pub use ids::{RuntimeSessionId, RuntimeTurnId};
pub use rara_core::llm::{
    backend::{LlmBackend, LlmTurnMetadata},
    contracts::LlmStreamEvent,
    types::{ContentBlock, LlmResponse, Message, TokenUsage},
};
pub use rara_core::tool::{
    Tool, ToolCallContext, ToolError, ToolManager, ToolOutputStream, ToolProgressEvent,
};
pub use session::{
    RuntimeEventStream, RuntimeSession, RuntimeSessionBuilder, RuntimeSessionSnapshot,
    RuntimeSessionSubscription,
};
pub use snapshot::{RuntimeSessionPhase, SessionSnapshot};
pub use turn::{RuntimeTurn, RuntimeTurnOutcome};
