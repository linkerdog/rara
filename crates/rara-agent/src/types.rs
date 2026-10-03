use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    #[default]
    Execute,
    Plan,
    Review,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopProgress {
    /// Tool iterations and forced continuations admitted in this outer query.
    pub agentic_turns: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IterationBudget {
    pub max_turns: Option<usize>,
    pub token_budget: Option<u32>,
    pub total_model_tokens: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseEvidence {
    pub had_text_response: bool,
    pub had_reasoning_response: bool,
}

impl ResponseEvidence {
    pub fn is_reasoning_only(self) -> bool {
        self.had_reasoning_response && !self.had_text_response
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelObservation {
    pub tool_call_count: usize,
    pub plan_exit_call_id: Option<String>,
    pub plan_updated: bool,
    pub malformed_proposed_plan: bool,
    pub continue_inspection: bool,
    pub response: ResponseEvidence,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionEvidence {
    pub has_any_evidence: bool,
    pub has_minimum_review_evidence: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInteractions {
    pub user_input: bool,
    pub approval: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationContext {
    pub mode: ExecutionMode,
    pub plan_steps: usize,
    pub inspection: InspectionEvidence,
    pub pending: PendingInteractions,
}

impl ContinuationContext {
    pub(crate) fn continuation(
        self,
        observation: &ModelObservation,
        progress: LoopProgress,
    ) -> Option<Continuation> {
        if self.pending.user_input || self.pending.approval {
            return None;
        }
        let reasoning_only = observation.response.is_reasoning_only();
        match self.mode {
            ExecutionMode::Plan => {
                let shallow_initial_plan =
                    observation.plan_updated && progress.agentic_turns == 0 && self.plan_steps <= 1;
                let missing_inspection = progress.agentic_turns > 0
                    && !observation.plan_updated
                    && self.inspection.has_any_evidence
                    && !self.inspection.has_minimum_review_evidence;
                let should_continue = (observation.continue_inspection
                    || shallow_initial_plan
                    || missing_inspection
                    || reasoning_only)
                    && (observation.continue_inspection
                        || self.inspection.has_any_evidence
                        || self.plan_steps > 0
                        || observation.response.had_text_response
                        || reasoning_only);
                should_continue.then_some(if reasoning_only {
                    Continuation::Automatic(TextContinuation::PlanReasoningOnly)
                } else {
                    Continuation::Automatic(TextContinuation::PlanNeedsEvidence)
                })
            }
            ExecutionMode::Execute => (observation.continue_inspection || reasoning_only)
                .then_some(if reasoning_only {
                    Continuation::Automatic(TextContinuation::ExecuteReasoningOnly)
                } else {
                    Continuation::Automatic(TextContinuation::ExecuteNeedsInspection)
                }),
            ExecutionMode::Review => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanExitIssue {
    IncompletePlan,
    MissingPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanExitRejection {
    pub call_id: String,
    pub issue: PlanExitIssue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Continuation {
    PlanExitRepair(PlanExitRejection),
    Automatic(TextContinuation),
    StopHookBlocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextContinuation {
    PlanNeedsEvidence,
    PlanReasoningOnly,
    ExecuteNeedsInspection,
    ExecuteReasoningOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopHookOutcome {
    AllowCompletion,
    BlockCompletion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolBatchOutcome {
    ResultsAvailable,
    AwaitingApproval,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopEnd {
    ResponseComplete,
    StopHookLimitReached { limit: usize },
    MaxTurnsReached { limit: usize },
    TokenBudgetReached { budget: u32, used: u32 },
    PlanExitRepairExhausted(PlanExitRejection),
    AwaitingApproval,
}

/// Work the host must finish before acknowledging the corresponding input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopEffect {
    RequestModel,
    RecordAssistant,
    Continue(Continuation),
    RunStopHooks { stop_hook_active: bool },
    RunTools,
    CommitToolResults,
    Finalize(LoopEnd),
}

/// Identity of one issued effect; acknowledgements must return it unchanged.
/// Scoped to one machine instance; hosts must also preserve session/turn identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectId(pub(crate) u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopRequest {
    pub id: EffectId,
    pub effect: LoopEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopPhase {
    Ready,
    ModelPending,
    AssistantPending,
    StopHooksPending,
    ToolsPending,
    ToolResultsPending,
    ContinuationPending,
    Finalizing,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopInput {
    BeginIteration,
    ModelCompleted,
    AssistantRecorded,
    StopHooksCompleted,
    ToolsCompleted,
    CheckpointCompleted,
    FinalizationCompleted,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("cannot apply {input:?} while the loop is {phase:?}")]
    WrongPhase { input: LoopInput, phase: LoopPhase },
    #[error("expected effect {expected:?}, received acknowledgement for {received:?}")]
    WrongEffect {
        expected: EffectId,
        received: EffectId,
    },
    #[error("agentic turn counter overflow")]
    CounterOverflow,
    #[error("effect identity counter overflow")]
    EffectCounterOverflow,
}
