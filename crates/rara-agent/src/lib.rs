//! Shared agent-loop transitions and runtime-independent effect execution.

mod executor;
mod history;
mod machine;
mod model;
mod tools;
mod types;

pub use executor::{LoopEffects, StopHookContext, execute_loop};
pub use history::{
    has_tool_result_block, keep_or_drop_tool_results, repair_tool_result_history,
    synthetic_tool_result_blocks, tool_use_ids_in_blocks,
};
pub use machine::LoopMachine;
pub use model::{
    ModelRequest, ModelTurnEvent, ModelTurnOutput, ModelTurnPolicy, StreamEvidence, ToolCall,
    execute_model_turn,
};
pub use tools::{
    ToolAdmission, ToolBatchEffects, ToolBatchOutput, ToolCallProgress, ToolCallProgressCallback,
    ToolReply, execute_tool_batch, execute_tool_call,
};
pub use types::{
    Continuation, ContinuationContext, EffectId, ExecutionMode, InspectionEvidence,
    IterationBudget, LoopEffect, LoopEnd, LoopInput, LoopPhase, LoopProgress, LoopRequest,
    ModelObservation, PendingInteractions, PlanExitIssue, PlanExitRejection, ResponseEvidence,
    StopHookOutcome, TextContinuation, ToolBatchOutcome, TransitionError,
};

#[cfg(test)]
mod tests;
