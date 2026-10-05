use super::*;

fn next_after_text(
    context: ContinuationContext,
    observation: ModelObservation,
    progress: LoopProgress,
) -> Result<LoopEffect, TransitionError> {
    let mut machine = LoopMachine::new(progress);
    let model = machine.begin_iteration(IterationBudget::default())?;
    let assistant = machine.model_completed(model.id, observation)?;
    Ok(machine.assistant_recorded(assistant.id, context)?.effect)
}

#[test]
fn continuation_modes_preserve_reasoning_inspection_and_pending_input_rules() -> TestResult {
    for mode in [
        ExecutionMode::Execute,
        ExecutionMode::Plan,
        ExecutionMode::Review,
    ] {
        for pending in [
            PendingInteractions::default(),
            PendingInteractions {
                user_input: true,
                approval: false,
            },
            PendingInteractions {
                user_input: false,
                approval: true,
            },
        ] {
            let context = ContinuationContext {
                mode,
                pending,
                ..Default::default()
            };
            for observation in [
                ModelObservation {
                    response: ResponseEvidence {
                        had_text_response: false,
                        had_reasoning_response: true,
                    },
                    ..Default::default()
                },
                ModelObservation {
                    continue_inspection: true,
                    response: ResponseEvidence {
                        had_text_response: true,
                        had_reasoning_response: false,
                    },
                    ..Default::default()
                },
                ModelObservation {
                    response: ResponseEvidence {
                        had_text_response: true,
                        had_reasoning_response: true,
                    },
                    ..Default::default()
                },
            ] {
                let expected = match (
                    mode,
                    pending.user_input || pending.approval,
                    observation.response.is_reasoning_only(),
                    observation.continue_inspection,
                ) {
                    (ExecutionMode::Execute, false, true, _) => LoopEffect::Continue(
                        Continuation::Automatic(TextContinuation::ExecuteReasoningOnly),
                    ),
                    (ExecutionMode::Plan, false, true, _) => LoopEffect::Continue(
                        Continuation::Automatic(TextContinuation::PlanReasoningOnly),
                    ),
                    (ExecutionMode::Execute, false, false, true) => LoopEffect::Continue(
                        Continuation::Automatic(TextContinuation::ExecuteNeedsInspection),
                    ),
                    (ExecutionMode::Plan, false, false, true) => LoopEffect::Continue(
                        Continuation::Automatic(TextContinuation::PlanNeedsEvidence),
                    ),
                    _ => LoopEffect::RunStopHooks {
                        stop_hook_active: false,
                    },
                };
                assert_eq!(
                    next_after_text(context, observation, LoopProgress::default())?,
                    expected
                );
            }
        }
    }
    Ok(())
}

#[test]
fn shallow_initial_plan_and_incomplete_inspection_keep_existing_plan_rules() -> TestResult {
    let context = ContinuationContext {
        mode: ExecutionMode::Plan,
        plan_steps: 1,
        ..Default::default()
    };
    let observation = ModelObservation {
        plan_updated: true,
        response: ResponseEvidence {
            had_text_response: true,
            had_reasoning_response: false,
        },
        ..Default::default()
    };
    assert_eq!(
        next_after_text(context, observation.clone(), LoopProgress::default())?,
        LoopEffect::Continue(Continuation::Automatic(TextContinuation::PlanNeedsEvidence))
    );
    assert_eq!(
        next_after_text(context, observation, LoopProgress { agentic_turns: 1 })?,
        LoopEffect::RunStopHooks {
            stop_hook_active: false
        }
    );
    let context = ContinuationContext {
        mode: ExecutionMode::Plan,
        inspection: InspectionEvidence {
            has_any_evidence: true,
            has_minimum_review_evidence: false,
        },
        ..Default::default()
    };
    assert_eq!(
        next_after_text(
            context,
            ModelObservation::default(),
            LoopProgress { agentic_turns: 1 }
        )?,
        LoopEffect::Continue(Continuation::Automatic(TextContinuation::PlanNeedsEvidence))
    );
    assert_eq!(
        next_after_text(
            ContinuationContext {
                inspection: InspectionEvidence {
                    has_any_evidence: true,
                    has_minimum_review_evidence: true
                },
                ..context
            },
            ModelObservation::default(),
            LoopProgress { agentic_turns: 1 }
        )?,
        LoopEffect::RunStopHooks {
            stop_hook_active: false
        }
    );
    Ok(())
}
