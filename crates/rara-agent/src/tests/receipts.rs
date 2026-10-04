use super::*;

fn restored_step<T: std::fmt::Debug + PartialEq>(
    machine: &mut LoopMachine,
    apply: impl Fn(&mut LoopMachine) -> Result<T, TransitionError>,
) -> Result<T, Box<dyn std::error::Error>> {
    let mut restored: LoopMachine = serde_json::from_slice(&serde_json::to_vec(machine)?)?;
    assert_eq!(*machine, restored);
    let expected = apply(machine)?;
    assert_eq!(expected, apply(&mut restored)?);
    assert_eq!(*machine, restored);
    Ok(expected)
}

#[test]
fn round_trips_preserve_transitions_through_tools_hooks_and_finalization() -> TestResult {
    let mut machine = LoopMachine::new(LoopProgress::default());
    let model = restored_step(&mut machine, |m| {
        m.begin_iteration(IterationBudget::default())
    })?;
    let assistant = restored_step(&mut machine, |m| {
        m.model_completed(
            model.id,
            ModelObservation {
                tool_call_count: 1,
                ..Default::default()
            },
        )
    })?;
    let tools = restored_step(&mut machine, |m| {
        m.assistant_recorded(assistant.id, ContinuationContext::default())
    })?;
    let results = restored_step(&mut machine, |m| {
        m.tools_completed(tools.id, ToolBatchOutcome::ResultsAvailable)
    })?;
    let model = restored_step(&mut machine, |m| {
        m.checkpoint_completed(results.id, IterationBudget::default())
    })?;
    let assistant = restored_step(&mut machine, |m| {
        m.model_completed(model.id, ModelObservation::default())
    })?;
    let hooks = restored_step(&mut machine, |m| {
        m.assistant_recorded(assistant.id, ContinuationContext::default())
    })?;
    let finish = restored_step(&mut machine, |m| {
        m.stop_hooks_completed(hooks.id, StopHookOutcome::AllowCompletion)
    })?;
    assert_eq!(
        restored_step(&mut machine, |m| m.finalization_completed(finish.id))?,
        LoopEnd::ResponseComplete
    );
    let restored: LoopMachine = serde_json::from_slice(&serde_json::to_vec(&machine)?)?;
    assert_eq!(machine, restored);
    assert_eq!(restored.phase(), LoopPhase::Finished);
    Ok(())
}

#[test]
fn stale_tool_and_wrong_phase_receipts_cannot_advance_a_later_batch() -> TestResult {
    let mut machine = LoopMachine::new(LoopProgress::default());
    let mut model = machine.begin_iteration(IterationBudget::default())?;
    let mut previous_tools = None;
    for _ in 0..2 {
        let assistant = machine.model_completed(
            model.id,
            ModelObservation {
                tool_call_count: 1,
                ..Default::default()
            },
        )?;
        let tools = machine.assistant_recorded(assistant.id, ContinuationContext::default())?;
        let before = machine.clone();
        if let Some(old) = previous_tools {
            assert!(matches!(
                machine.tools_completed(old, ToolBatchOutcome::ResultsAvailable),
                Err(TransitionError::WrongEffect { .. })
            ));
        }
        assert_eq!(
            machine.checkpoint_completed(tools.id, IterationBudget::default()),
            Err(TransitionError::WrongPhase {
                input: LoopInput::CheckpointCompleted,
                phase: LoopPhase::ToolsPending,
            })
        );
        assert_eq!(machine, before);
        let results = machine.tools_completed(tools.id, ToolBatchOutcome::ResultsAvailable)?;
        model = machine.checkpoint_completed(results.id, IterationBudget::default())?;
        previous_tools = Some(tools.id);
    }
    assert_eq!(machine.progress().agentic_turns, 2);
    Ok(())
}

#[test]
fn exhausted_effect_identity_fails_without_advancing_control_state() -> TestResult {
    let mut machine = LoopMachine::new(LoopProgress::default());
    machine.begin_iteration(IterationBudget::default())?;
    let mut snapshot = serde_json::to_value(machine)?;
    snapshot["effect_id"] = serde_json::json!(u64::MAX);
    let mut machine: LoopMachine = serde_json::from_value(snapshot)?;
    let before = machine.clone();
    assert_eq!(
        machine.model_completed(EffectId(u64::MAX), ModelObservation::default()),
        Err(TransitionError::EffectCounterOverflow)
    );
    assert_eq!(machine, before);
    Ok(())
}
