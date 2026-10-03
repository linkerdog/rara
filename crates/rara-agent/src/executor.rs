use anyhow::Result;
use async_trait::async_trait;

use crate::{
    Continuation, ContinuationContext, IterationBudget, LoopEffect, LoopEnd, LoopMachine,
    LoopProgress, LoopRequest, ModelObservation, StopHookOutcome, ToolBatchOutcome,
};

/// Context passed to the host's completion hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopHookContext {
    pub stop_hook_active: bool,
}

/// Host effects for the shared executor, with one session-scoped owner.
///
/// Return success only after the requested effect and its required checkpoint
/// or cleanup have completed. Preserve model/tool identity in host-owned data.
/// Errors must retain diagnostic information; the executor never retries an
/// effect or finalizes a failed one. Cancellation cleanup belongs to the host.
#[async_trait]
pub trait LoopEffects: Send {
    fn budget(&self) -> IterationBudget;

    async fn request_model(&mut self, progress: LoopProgress) -> Result<ModelObservation>;

    async fn record_assistant(&mut self, progress: LoopProgress) -> Result<ContinuationContext>;

    async fn continue_turn(
        &mut self,
        continuation: Continuation,
        progress: LoopProgress,
    ) -> Result<()>;

    async fn run_stop_hooks(
        &mut self,
        context: StopHookContext,
        progress: LoopProgress,
    ) -> Result<StopHookOutcome>;

    async fn run_tools(&mut self, progress: LoopProgress) -> Result<ToolBatchOutcome>;

    async fn commit_tool_results(&mut self, progress: LoopProgress) -> Result<()>;

    async fn finalize(&mut self, end: LoopEnd, progress: LoopProgress) -> Result<()>;
}

/// Drive the shared machine without choosing an async runtime or native policy.
///
/// Progress is published before each effect, including effects that fail. A
/// successful return means the host acknowledged finalization; dropping this
/// future provides no cleanup or completion guarantee.
pub async fn execute_loop(
    effects: &mut (impl LoopEffects + ?Sized),
    progress: &mut LoopProgress,
) -> Result<LoopEnd> {
    let mut machine = LoopMachine::new(*progress);
    let mut request = machine.begin_iteration(effects.budget())?;
    loop {
        *progress = machine.progress();
        let LoopRequest { id, effect } = request;
        request = match effect {
            LoopEffect::RequestModel => {
                machine.model_completed(id, effects.request_model(*progress).await?)?
            }
            LoopEffect::RecordAssistant => {
                machine.assistant_recorded(id, effects.record_assistant(*progress).await?)?
            }
            LoopEffect::Continue(continuation) => {
                effects.continue_turn(continuation, *progress).await?;
                machine.checkpoint_completed(id, effects.budget())?
            }
            LoopEffect::RunStopHooks { stop_hook_active } => machine.stop_hooks_completed(
                id,
                effects
                    .run_stop_hooks(StopHookContext { stop_hook_active }, *progress)
                    .await?,
            )?,
            LoopEffect::RunTools => {
                machine.tools_completed(id, effects.run_tools(*progress).await?)?
            }
            LoopEffect::CommitToolResults => {
                effects.commit_tool_results(*progress).await?;
                machine.checkpoint_completed(id, effects.budget())?
            }
            LoopEffect::Finalize(end) => {
                effects.finalize(end, *progress).await?;
                return Ok(machine.finalization_completed(id)?);
            }
        };
    }
}

#[cfg(test)]
mod tests;
