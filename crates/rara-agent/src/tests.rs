use crate::*;

mod continuation;
mod receipts;
mod recovery;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn tool_effects_require_ordered_commit_and_finalization_receipts() -> TestResult {
    let mut machine = LoopMachine::new(LoopProgress::default());
    let model = machine.begin_iteration(IterationBudget::default())?;
    assert_eq!(model.effect, LoopEffect::RequestModel);
    let assistant = machine.model_completed(
        model.id,
        ModelObservation {
            tool_call_count: 2,
            ..ModelObservation::default()
        },
    )?;
    assert_eq!(assistant.effect, LoopEffect::RecordAssistant);
    let tools = machine.assistant_recorded(assistant.id, ContinuationContext::default())?;
    assert_eq!(tools.effect, LoopEffect::RunTools);
    assert_eq!(machine.progress().agentic_turns, 1);
    let results = machine.tools_completed(tools.id, ToolBatchOutcome::ResultsAvailable)?;
    assert_eq!(results.effect, LoopEffect::CommitToolResults);
    let snapshot = machine.clone();
    assert!(
        machine
            .model_completed(model.id, ModelObservation::default())
            .is_err()
    );
    assert_eq!(machine, snapshot);
    let next = machine.checkpoint_completed(results.id, IterationBudget::default())?;
    assert_eq!(next.effect, LoopEffect::RequestModel);
    let snapshot = machine.clone();
    assert!(matches!(
        machine.model_completed(model.id, ModelObservation::default()),
        Err(TransitionError::WrongEffect { .. })
    ));
    assert_eq!(machine, snapshot);
    let assistant = machine.model_completed(next.id, ModelObservation::default())?;
    let hooks = machine.assistant_recorded(assistant.id, ContinuationContext::default())?;
    assert_eq!(
        hooks.effect,
        LoopEffect::RunStopHooks {
            stop_hook_active: false
        }
    );
    let finish = machine.stop_hooks_completed(hooks.id, StopHookOutcome::AllowCompletion)?;
    assert_eq!(
        finish.effect,
        LoopEffect::Finalize(LoopEnd::ResponseComplete)
    );
    assert_eq!(machine.phase(), LoopPhase::Finalizing);
    assert_eq!(
        machine.finalization_completed(finish.id)?,
        LoopEnd::ResponseComplete
    );
    assert_eq!(machine.phase(), LoopPhase::Finished);
    assert!(machine.finalization_completed(finish.id).is_err());
    assert_eq!(machine.progress().agentic_turns, 1);
    Ok(())
}

#[test]
fn pending_control_state_round_trip_retains_effect_identity() -> TestResult {
    let mut original = LoopMachine::new(LoopProgress::default());
    let model = original.begin_iteration(IterationBudget::default())?;
    let mut restored: LoopMachine = serde_json::from_slice(&serde_json::to_vec(&original)?)?;
    assert_eq!(original, restored);
    let observation = ModelObservation {
        response: ResponseEvidence {
            had_text_response: true,
            had_reasoning_response: false,
        },
        ..ModelObservation::default()
    };
    assert_eq!(
        original.model_completed(model.id, observation.clone())?,
        restored.model_completed(model.id, observation)?
    );
    assert_eq!(original, restored);
    Ok(())
}
