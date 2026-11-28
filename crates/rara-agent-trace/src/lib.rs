//! Content-free, session-scoped agent trace records and local persistence.

mod model;
mod recorder;

pub use model::{
    AGENT_TRACE_SCHEMA_VERSION, AgentStepUpdated, AgentTraceEvent, CacheUsage, ContextAssembled,
    ModelFinished, ModelUsage, TraceManifest, TraceModelStatus, TraceRecord, TraceTurnOutcome,
    TurnFinished, TurnStarted,
};
pub use recorder::{AgentTraceLocation, AgentTraceRecorder};
