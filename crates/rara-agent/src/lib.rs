//! Deterministic agent-loop transitions, independent of execution and storage.

mod machine;
mod types;

pub use machine::LoopMachine;
pub use types::{
    Continuation, ContinuationContext, EffectId, ExecutionMode, InspectionEvidence,
    IterationBudget, LoopEffect, LoopEnd, LoopInput, LoopPhase, LoopProgress, LoopRequest,
    ModelObservation, PendingInteractions, PlanExitIssue, PlanExitRejection, ResponseEvidence,
    StopHookOutcome, TextContinuation, ToolBatchOutcome, TransitionError,
};

#[cfg(test)]
mod tests;
