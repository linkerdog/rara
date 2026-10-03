use super::*;

fn no_tool_response(
    machine: &mut LoopMachine,
    request: LoopRequest,
) -> Result<LoopRequest, TransitionError> {
    let assistant = machine.model_completed(request.id, ModelObservation::default())?;
    machine.assistant_recorded(assistant.id, ContinuationContext::default())
}

#[test]
fn limits_apply_before_the_next_model_and_turn_limit_wins() -> TestResult {
    for (budget, expected) in [
        (
            IterationBudget {
                max_turns: Some(0),
                token_budget: Some(0),
                total_model_tokens: 0,
            },
            LoopEnd::MaxTurnsReached { limit: 0 },
        ),
        (
            IterationBudget {
                max_turns: None,
                token_budget: Some(10),
                total_model_tokens: 10,
            },
            LoopEnd::TokenBudgetReached {
                budget: 10,
                used: 10,
            },
        ),
    ] {
        let mut machine = LoopMachine::new(LoopProgress::default());
        assert_eq!(
            machine.begin_iteration(budget)?.effect,
            LoopEffect::Finalize(expected)
        );
        assert_eq!(machine.phase(), LoopPhase::Finalizing);
    }
    let mut machine = LoopMachine::new(LoopProgress::default());
    let model = machine.begin_iteration(IterationBudget {
        max_turns: Some(1),
        ..Default::default()
    })?;
    let assistant = machine.model_completed(
        model.id,
        ModelObservation {
            tool_call_count: 1,
            ..Default::default()
        },
    )?;
    let tools = machine.assistant_recorded(assistant.id, ContinuationContext::default())?;
    let results = machine.tools_completed(tools.id, ToolBatchOutcome::ResultsAvailable)?;
    let finish = machine.checkpoint_completed(
        results.id,
        IterationBudget {
            max_turns: Some(1),
            ..Default::default()
        },
    )?;
    assert_eq!(
        finish.effect,
        LoopEffect::Finalize(LoopEnd::MaxTurnsReached { limit: 1 })
    );
    Ok(())
}

#[test]
fn plan_exit_repair_rejects_before_recording_and_stops_after_one_attempt() -> TestResult {
    let mut machine = LoopMachine::new(LoopProgress::default());
    let model = machine.begin_iteration(IterationBudget::default())?;
    let observation = ModelObservation {
        tool_call_count: 1,
        plan_exit_call_id: Some("exit-1".into()),
        malformed_proposed_plan: true,
        ..Default::default()
    };
    let rejection = PlanExitRejection {
        call_id: "exit-1".into(),
        issue: PlanExitIssue::IncompletePlan,
    };
    let repair = machine.model_completed(model.id, observation.clone())?;
    assert_eq!(
        repair.effect,
        LoopEffect::Continue(Continuation::PlanExitRepair(rejection.clone()))
    );
    assert_eq!(machine.progress().agentic_turns, 1);
    let mut restored: LoopMachine = serde_json::from_slice(&serde_json::to_vec(&machine)?)?;
    let model = restored.checkpoint_completed(repair.id, IterationBudget::default())?;
    let stopped = restored.model_completed(model.id, observation)?;
    assert_eq!(
        stopped.effect,
        LoopEffect::Finalize(LoopEnd::PlanExitRepairExhausted(rejection))
    );
    assert_eq!(restored.progress().agentic_turns, 1);
    Ok(())
}

#[test]
fn stop_hook_limit_survives_serialization_and_does_not_count_final_response() -> TestResult {
    let mut machine = LoopMachine::new(LoopProgress::default());
    let mut request = machine.begin_iteration(IterationBudget::default())?;
    for index in 0..8 {
        let hooks = no_tool_response(&mut machine, request)?;
        assert_eq!(
            hooks.effect,
            LoopEffect::RunStopHooks {
                stop_hook_active: index > 0
            }
        );
        let continued = machine.stop_hooks_completed(hooks.id, StopHookOutcome::BlockCompletion)?;
        assert_eq!(
            continued.effect,
            LoopEffect::Continue(Continuation::StopHookBlocked)
        );
        machine = serde_json::from_slice(&serde_json::to_vec(&machine)?)?;
        request = machine.checkpoint_completed(continued.id, IterationBudget::default())?;
    }
    let hooks = no_tool_response(&mut machine, request)?;
    let finish = machine.stop_hooks_completed(hooks.id, StopHookOutcome::BlockCompletion)?;
    assert_eq!(
        finish.effect,
        LoopEffect::Finalize(LoopEnd::StopHookLimitReached { limit: 8 })
    );
    assert_eq!(machine.progress().agentic_turns, 8);
    Ok(())
}

#[test]
fn approval_pauses_before_tool_result_commit_or_stop_hooks() -> TestResult {
    let mut machine = LoopMachine::new(LoopProgress::default());
    let model = machine.begin_iteration(IterationBudget::default())?;
    let assistant = machine.model_completed(
        model.id,
        ModelObservation {
            tool_call_count: 1,
            ..Default::default()
        },
    )?;
    let tools = machine.assistant_recorded(assistant.id, ContinuationContext::default())?;
    let finish = machine.tools_completed(tools.id, ToolBatchOutcome::AwaitingApproval)?;
    assert_eq!(
        finish.effect,
        LoopEffect::Finalize(LoopEnd::AwaitingApproval)
    );
    let before = machine.clone();
    assert!(
        machine
            .checkpoint_completed(finish.id, IterationBudget::default())
            .is_err()
    );
    assert_eq!(machine, before);
    assert_eq!(
        machine.finalization_completed(finish.id)?,
        LoopEnd::AwaitingApproval
    );
    Ok(())
}

#[test]
fn counter_overflow_does_not_partially_advance_the_machine() -> TestResult {
    let mut machine = LoopMachine::new(LoopProgress {
        agentic_turns: usize::MAX,
    });
    let model = machine.begin_iteration(IterationBudget::default())?;
    let assistant = machine.model_completed(
        model.id,
        ModelObservation {
            tool_call_count: 1,
            ..Default::default()
        },
    )?;
    let before = machine.clone();
    assert_eq!(
        machine.assistant_recorded(assistant.id, ContinuationContext::default()),
        Err(TransitionError::CounterOverflow)
    );
    assert_eq!(machine, before);
    Ok(())
}
