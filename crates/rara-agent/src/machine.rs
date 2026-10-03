use serde::{Deserialize, Serialize};

use crate::{
    Continuation, ContinuationContext, EffectId, IterationBudget, LoopEffect, LoopEnd, LoopInput,
    LoopPhase, LoopProgress, LoopRequest, ModelObservation, PlanExitIssue, PlanExitRejection,
    StopHookOutcome, ToolBatchOutcome, TransitionError,
};

const MAX_PLAN_EXIT_REPAIRS: usize = 1;
const MAX_STOP_HOOK_CONTINUATIONS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum State {
    Ready,
    ModelPending,
    AssistantPending(ModelObservation),
    StopHooksPending,
    ToolsPending,
    ToolResultsPending,
    ContinuationPending(Continuation),
    Finalizing(LoopEnd),
    Finished(LoopEnd),
}

/// Serializable control state for one entry into the shared agent loop.
///
/// The host owns transcript data and external effects. A snapshot does not
/// authorize repeating an effect or replace the host's durable effect ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopMachine {
    state: State,
    effect_id: EffectId,
    progress: LoopProgress,
    plan_exit_repairs: usize,
    stop_hook_continuations: usize,
}

impl LoopMachine {
    pub fn new(progress: LoopProgress) -> Self {
        Self {
            state: State::Ready,
            effect_id: EffectId(0),
            progress,
            plan_exit_repairs: 0,
            stop_hook_continuations: 0,
        }
    }

    pub fn progress(&self) -> LoopProgress {
        self.progress
    }

    pub fn phase(&self) -> LoopPhase {
        match self.state {
            State::Ready => LoopPhase::Ready,
            State::ModelPending => LoopPhase::ModelPending,
            State::AssistantPending(_) => LoopPhase::AssistantPending,
            State::StopHooksPending => LoopPhase::StopHooksPending,
            State::ToolsPending => LoopPhase::ToolsPending,
            State::ToolResultsPending => LoopPhase::ToolResultsPending,
            State::ContinuationPending(_) => LoopPhase::ContinuationPending,
            State::Finalizing(_) => LoopPhase::Finalizing,
            State::Finished(_) => LoopPhase::Finished,
        }
    }

    pub fn begin_iteration(
        &mut self,
        budget: IterationBudget,
    ) -> Result<LoopRequest, TransitionError> {
        self.require_phase(LoopPhase::Ready, LoopInput::BeginIteration)?;
        Ok(self.next_iteration(self.next_effect_id()?, budget))
    }

    pub fn model_completed(
        &mut self,
        receipt: EffectId,
        observation: ModelObservation,
    ) -> Result<LoopRequest, TransitionError> {
        self.require_effect(receipt)?;
        self.require_phase(LoopPhase::ModelPending, LoopInput::ModelCompleted)?;
        let next = self.next_effect_id()?;
        if let Some(call_id) = &observation.plan_exit_call_id
            && (observation.malformed_proposed_plan || !observation.plan_updated)
        {
            let rejection = PlanExitRejection {
                call_id: call_id.clone(),
                issue: if observation.malformed_proposed_plan {
                    PlanExitIssue::IncompletePlan
                } else {
                    PlanExitIssue::MissingPlan
                },
            };
            if self.plan_exit_repairs < MAX_PLAN_EXIT_REPAIRS {
                self.increment_turn()?;
                self.plan_exit_repairs += 1;
                return Ok(self.continue_with(next, Continuation::PlanExitRepair(rejection)));
            }
            return Ok(self.finalize(next, LoopEnd::PlanExitRepairExhausted(rejection)));
        }
        self.state = State::AssistantPending(observation);
        Ok(self.request(next, LoopEffect::RecordAssistant))
    }

    pub fn assistant_recorded(
        &mut self,
        receipt: EffectId,
        context: ContinuationContext,
    ) -> Result<LoopRequest, TransitionError> {
        self.require_effect(receipt)?;
        let next = self.next_effect_id()?;
        let State::AssistantPending(observation) = &self.state else {
            return Err(self.wrong_phase(LoopInput::AssistantRecorded));
        };
        if observation.tool_call_count > 0 {
            self.increment_turn()?;
            self.state = State::ToolsPending;
            return Ok(self.request(next, LoopEffect::RunTools));
        }
        if let Some(continuation) = context.continuation(observation, self.progress) {
            self.increment_turn()?;
            return Ok(self.continue_with(next, continuation));
        }
        self.state = State::StopHooksPending;
        Ok(self.request(
            next,
            LoopEffect::RunStopHooks {
                stop_hook_active: self.stop_hook_continuations > 0,
            },
        ))
    }

    pub fn stop_hooks_completed(
        &mut self,
        receipt: EffectId,
        outcome: StopHookOutcome,
    ) -> Result<LoopRequest, TransitionError> {
        self.require_effect(receipt)?;
        self.require_phase(LoopPhase::StopHooksPending, LoopInput::StopHooksCompleted)?;
        let next = self.next_effect_id()?;
        match outcome {
            StopHookOutcome::AllowCompletion => Ok(self.finalize(next, LoopEnd::ResponseComplete)),
            StopHookOutcome::BlockCompletion => {
                if self.stop_hook_continuations < MAX_STOP_HOOK_CONTINUATIONS {
                    self.increment_turn()?;
                    self.stop_hook_continuations += 1;
                    Ok(self.continue_with(next, Continuation::StopHookBlocked))
                } else {
                    Ok(self.finalize(
                        next,
                        LoopEnd::StopHookLimitReached {
                            limit: MAX_STOP_HOOK_CONTINUATIONS,
                        },
                    ))
                }
            }
        }
    }

    pub fn tools_completed(
        &mut self,
        receipt: EffectId,
        outcome: ToolBatchOutcome,
    ) -> Result<LoopRequest, TransitionError> {
        self.require_effect(receipt)?;
        self.require_phase(LoopPhase::ToolsPending, LoopInput::ToolsCompleted)?;
        let next = self.next_effect_id()?;
        match outcome {
            ToolBatchOutcome::ResultsAvailable => {
                self.state = State::ToolResultsPending;
                Ok(self.request(next, LoopEffect::CommitToolResults))
            }
            ToolBatchOutcome::AwaitingApproval => {
                Ok(self.finalize(next, LoopEnd::AwaitingApproval))
            }
        }
    }

    /// Admit another iteration only after continuation/results are committed.
    pub fn checkpoint_completed(
        &mut self,
        receipt: EffectId,
        budget: IterationBudget,
    ) -> Result<LoopRequest, TransitionError> {
        self.require_effect(receipt)?;
        match self.state {
            State::ContinuationPending(_) | State::ToolResultsPending => {
                Ok(self.next_iteration(self.next_effect_id()?, budget))
            }
            State::Ready
            | State::ModelPending
            | State::AssistantPending(_)
            | State::StopHooksPending
            | State::ToolsPending
            | State::Finalizing(_)
            | State::Finished(_) => Err(self.wrong_phase(LoopInput::CheckpointCompleted)),
        }
    }

    /// A finalization request is not proof that host cleanup has completed.
    pub fn finalization_completed(
        &mut self,
        receipt: EffectId,
    ) -> Result<LoopEnd, TransitionError> {
        self.require_effect(receipt)?;
        let State::Finalizing(end) = &self.state else {
            return Err(self.wrong_phase(LoopInput::FinalizationCompleted));
        };
        let end = end.clone();
        self.state = State::Finished(end.clone());
        Ok(end)
    }

    fn next_iteration(&mut self, next: EffectId, budget: IterationBudget) -> LoopRequest {
        if let Some(limit) = budget.max_turns
            && self.progress.agentic_turns >= limit
        {
            return self.finalize(next, LoopEnd::MaxTurnsReached { limit });
        }
        if let Some(limit) = budget.token_budget
            && budget.total_model_tokens >= limit
        {
            return self.finalize(
                next,
                LoopEnd::TokenBudgetReached {
                    budget: limit,
                    used: budget.total_model_tokens,
                },
            );
        }
        self.state = State::ModelPending;
        self.request(next, LoopEffect::RequestModel)
    }

    fn increment_turn(&mut self) -> Result<(), TransitionError> {
        self.progress.agentic_turns = self
            .progress
            .agentic_turns
            .checked_add(1)
            .ok_or(TransitionError::CounterOverflow)?;
        Ok(())
    }

    fn continue_with(&mut self, next: EffectId, continuation: Continuation) -> LoopRequest {
        self.state = State::ContinuationPending(continuation.clone());
        self.request(next, LoopEffect::Continue(continuation))
    }

    fn finalize(&mut self, next: EffectId, end: LoopEnd) -> LoopRequest {
        self.state = State::Finalizing(end.clone());
        self.request(next, LoopEffect::Finalize(end))
    }

    fn next_effect_id(&self) -> Result<EffectId, TransitionError> {
        self.effect_id
            .0
            .checked_add(1)
            .map(EffectId)
            .ok_or(TransitionError::EffectCounterOverflow)
    }

    fn request(&mut self, id: EffectId, effect: LoopEffect) -> LoopRequest {
        self.effect_id = id;
        LoopRequest { id, effect }
    }

    fn require_effect(&self, received: EffectId) -> Result<(), TransitionError> {
        if received == self.effect_id {
            Ok(())
        } else {
            Err(TransitionError::WrongEffect {
                expected: self.effect_id,
                received,
            })
        }
    }

    fn require_phase(&self, phase: LoopPhase, input: LoopInput) -> Result<(), TransitionError> {
        if self.phase() == phase {
            Ok(())
        } else {
            Err(self.wrong_phase(input))
        }
    }

    fn wrong_phase(&self, input: LoopInput) -> TransitionError {
        TransitionError::WrongPhase {
            input,
            phase: self.phase(),
        }
    }
}
