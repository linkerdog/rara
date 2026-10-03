use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context, Poll, Waker};

use super::*;
use crate::ResponseEvidence;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Model,
    Assistant,
    Tools,
    Commit,
    Continue,
    Hooks,
    Finalize,
}

#[derive(Clone, Copy, Debug, Default)]
enum Script {
    #[default]
    Tools,
    Reasoning,
}

#[derive(Default)]
struct Effects {
    released: Arc<AtomicUsize>,
    trace: Arc<Mutex<Vec<(Step, LoopProgress)>>>,
    fail_at: Option<usize>,
    script: Script,
    approval: bool,
    model_calls: usize,
    budget: IterationBudget,
    commit_budget: Option<IterationBudget>,
    final_outcome: Option<LoopEnd>,
}

impl Effects {
    async fn step(&mut self, step: Step, progress: LoopProgress) -> Result<()> {
        let index = {
            let mut trace = self
                .trace
                .lock()
                .map_err(|error| anyhow::anyhow!("test trace poisoned: {error}"))?;
            trace.push((step, progress));
            trace.len()
        };
        poll_fn(|_| {
            if self.released.load(Ordering::SeqCst) >= index {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        if self.fail_at == Some(index) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "host effect failed after cleanup",
            )
            .into());
        }
        Ok(())
    }
}

#[async_trait]
impl LoopEffects for Effects {
    fn budget(&self) -> IterationBudget {
        self.budget
    }

    async fn request_model(&mut self, progress: LoopProgress) -> Result<ModelObservation> {
        self.step(Step::Model, progress).await?;
        self.model_calls += 1;
        Ok(ModelObservation {
            tool_call_count: usize::from(
                self.model_calls == 1 && matches!(self.script, Script::Tools),
            ),
            response: ResponseEvidence {
                had_reasoning_response: self.model_calls == 1
                    && matches!(self.script, Script::Reasoning),
                had_text_response: self.model_calls > 1,
            },
            ..Default::default()
        })
    }

    async fn record_assistant(&mut self, progress: LoopProgress) -> Result<ContinuationContext> {
        self.step(Step::Assistant, progress).await?;
        Ok(ContinuationContext::default())
    }

    async fn continue_turn(&mut self, _: Continuation, progress: LoopProgress) -> Result<()> {
        self.step(Step::Continue, progress).await
    }

    async fn run_stop_hooks(
        &mut self,
        context: StopHookContext,
        progress: LoopProgress,
    ) -> Result<StopHookOutcome> {
        self.step(Step::Hooks, progress).await?;
        assert!(!context.stop_hook_active);
        Ok(StopHookOutcome::AllowCompletion)
    }

    async fn run_tools(&mut self, progress: LoopProgress) -> Result<ToolBatchOutcome> {
        self.step(Step::Tools, progress).await?;
        Ok(if self.approval {
            ToolBatchOutcome::AwaitingApproval
        } else {
            ToolBatchOutcome::ResultsAvailable
        })
    }

    async fn commit_tool_results(&mut self, progress: LoopProgress) -> Result<()> {
        self.step(Step::Commit, progress).await?;
        if let Some(budget) = self.commit_budget {
            self.budget = budget;
        }
        Ok(())
    }

    async fn finalize(&mut self, end: LoopEnd, progress: LoopProgress) -> Result<()> {
        self.step(Step::Finalize, progress).await?;
        self.final_outcome = Some(end);
        Ok(())
    }
}

const TOOL_SEQUENCE: [Step; 8] = [
    Step::Model,
    Step::Assistant,
    Step::Tools,
    Step::Commit,
    Step::Model,
    Step::Assistant,
    Step::Hooks,
    Step::Finalize,
];

#[test]
fn each_pending_effect_blocks_followup_and_completion_without_an_async_runtime() -> Result<()> {
    let mut effects = Effects::default();
    let trace = effects.trace.clone();
    let released = effects.released.clone();
    let mut progress = LoopProgress::default();
    {
        let mut future = pin!(execute_loop(
            &mut effects as &mut dyn LoopEffects,
            &mut progress
        ));
        let mut context = Context::from_waker(Waker::noop());
        for (index, expected) in TOOL_SEQUENCE.iter().enumerate() {
            released.store(index, Ordering::SeqCst);
            assert!(future.as_mut().poll(&mut context).is_pending());
            let trace = trace
                .lock()
                .map_err(|error| anyhow::anyhow!("test trace poisoned: {error}"))?;
            assert_eq!(trace.len(), index + 1);
            assert_eq!(
                trace[index],
                (
                    *expected,
                    LoopProgress {
                        agentic_turns: usize::from(index >= 2)
                    }
                )
            );
            assert!(future.as_mut().poll(&mut context).is_pending());
        }
        released.store(TOOL_SEQUENCE.len(), Ordering::SeqCst);
        assert!(matches!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Ok(LoopEnd::ResponseComplete))
        ));
    }
    assert_eq!(progress.agentic_turns, 1);
    assert_eq!(effects.final_outcome, Some(LoopEnd::ResponseComplete));
    Ok(())
}

#[test]
fn failed_effects_preserve_the_original_error_and_progress_without_replay() -> Result<()> {
    for fail_at in 1..=TOOL_SEQUENCE.len() {
        let mut effects = Effects {
            fail_at: Some(fail_at),
            ..Default::default()
        };
        effects.released.store(usize::MAX, Ordering::SeqCst);
        let mut progress = LoopProgress::default();
        let error = match pin!(execute_loop(&mut effects, &mut progress))
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(Err(error)) => error,
            other => anyhow::bail!("expected original effect failure, got {other:?}"),
        };
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .ok_or_else(|| anyhow::anyhow!("missing original I/O error"))?
                .kind(),
            std::io::ErrorKind::Interrupted
        );
        assert_eq!(progress.agentic_turns, usize::from(fail_at >= 3));
        let actual: Vec<_> = effects
            .trace
            .lock()
            .map_err(|error| anyhow::anyhow!("test trace poisoned: {error}"))?
            .iter()
            .map(|(step, _)| *step)
            .collect();
        assert_eq!(actual, TOOL_SEQUENCE[..fail_at]);
        assert_eq!(effects.final_outcome, None);
    }
    Ok(())
}

#[test]
fn approval_pause_awaits_finalization_without_committing_results_or_running_hooks() -> Result<()> {
    let mut effects = Effects {
        approval: true,
        ..Default::default()
    };
    effects.released.store(3, Ordering::SeqCst);
    let released = effects.released.clone();
    let mut progress = LoopProgress::default();
    {
        let mut future = pin!(execute_loop(&mut effects, &mut progress));
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        released.store(4, Ordering::SeqCst);
        assert!(matches!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Ok(LoopEnd::AwaitingApproval))
        ));
    }
    assert_eq!(effects.final_outcome, Some(LoopEnd::AwaitingApproval));
    let steps: Vec<_> = effects
        .trace
        .lock()
        .map_err(|error| anyhow::anyhow!("test trace poisoned: {error}"))?
        .iter()
        .map(|(step, _)| *step)
        .collect();
    assert_eq!(
        steps,
        [Step::Model, Step::Assistant, Step::Tools, Step::Finalize]
    );
    Ok(())
}

#[test]
fn next_request_uses_budget_after_the_tool_result_checkpoint() -> Result<()> {
    let mut effects = Effects {
        commit_budget: Some(IterationBudget {
            max_turns: Some(1),
            ..Default::default()
        }),
        ..Default::default()
    };
    effects.released.store(usize::MAX, Ordering::SeqCst);
    assert!(matches!(
        pin!(execute_loop(&mut effects, &mut LoopProgress::default()))
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(LoopEnd::MaxTurnsReached { limit: 1 }))
    ));
    assert_eq!(effects.model_calls, 1);
    let steps: Vec<_> = effects
        .trace
        .lock()
        .map_err(|error| anyhow::anyhow!("test trace poisoned: {error}"))?
        .iter()
        .map(|(step, _)| *step)
        .collect();
    assert_eq!(
        steps,
        [
            Step::Model,
            Step::Assistant,
            Step::Tools,
            Step::Commit,
            Step::Finalize
        ]
    );
    Ok(())
}

#[test]
fn failed_continuation_checkpoint_prevents_another_model_request() -> Result<()> {
    let mut effects = Effects {
        script: Script::Reasoning,
        fail_at: Some(3),
        ..Default::default()
    };
    effects.released.store(usize::MAX, Ordering::SeqCst);
    let mut progress = LoopProgress::default();
    assert!(matches!(
        pin!(execute_loop(&mut effects, &mut progress))
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(_))
    ));
    assert_eq!(effects.model_calls, 1);
    assert_eq!(progress.agentic_turns, 1);
    let steps: Vec<_> = effects
        .trace
        .lock()
        .map_err(|error| anyhow::anyhow!("test trace poisoned: {error}"))?
        .iter()
        .map(|(step, _)| *step)
        .collect();
    assert_eq!(steps, [Step::Model, Step::Assistant, Step::Continue]);
    Ok(())
}
