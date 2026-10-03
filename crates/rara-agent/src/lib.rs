//! Shared agent-loop transitions and runtime-independent effect execution.

mod executor;
mod machine;
mod model;
mod tools;
mod types;

pub use executor::{LoopEffects, StopHookContext, execute_loop};
pub use machine::LoopMachine;
pub use model::{
    ModelRequest, ModelTurnEvent, ModelTurnOutput, ModelTurnPolicy, StreamEvidence, ToolCall,
    execute_model_turn,
};
pub use tools::{
    ToolAdmission, ToolBatchEffects, ToolBatchOutput, ToolCallProgress, ToolReply,
    execute_tool_batch, execute_tool_call,
};
pub use types::{
    Continuation, ContinuationContext, EffectId, ExecutionMode, InspectionEvidence,
    IterationBudget, LoopEffect, LoopEnd, LoopInput, LoopPhase, LoopProgress, LoopRequest,
    ModelObservation, PendingInteractions, PlanExitIssue, PlanExitRejection, ResponseEvidence,
    StopHookOutcome, TextContinuation, ToolBatchOutcome, TransitionError,
};

#[cfg(test)]
mod tests;
