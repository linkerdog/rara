use rara_agent::{
    Continuation, ContinuationContext, InspectionEvidence, IterationBudget, LoopEffects, LoopEnd,
    LoopProgress, ModelObservation, PendingInteractions, PlanExitIssue, PlanExitRejection,
    ResponseEvidence, StopHookContext, StopHookOutcome, TextContinuation, ToolBatchOutcome,
    execute_loop,
};

use super::*;

struct LoopTurn {
    output: TurnOutput,
    last_assistant_message: Option<String>,
    assistant_message_recorded: bool,
    stop_hook_block: Option<StopHookBlock>,
    tool_results: Vec<Message>,
}

impl Agent {
    /// Run the shared executor with this session's native effects.
    pub(super) async fn run_agent_loop_with_limit<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
        agentic_turns: &mut usize,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut progress = LoopProgress {
            agentic_turns: *agentic_turns,
        };
        let result = execute_loop(
            &mut NativeLoopEffects {
                agent: self,
                report,
                output_mode,
                pending_turn: None,
            },
            &mut progress,
        )
        .await;
        // Preserve admitted work for outer recovery even when an effect fails.
        *agentic_turns = progress.agentic_turns;
        result.map(|_| ())
    }
}

struct NativeLoopEffects<'a, F> {
    agent: &'a mut Agent,
    report: &'a mut F,
    output_mode: AgentOutputMode,
    pending_turn: Option<LoopTurn>,
}

#[async_trait::async_trait]
impl<F> LoopEffects for NativeLoopEffects<'_, F>
where
    F: FnMut(AgentEvent) + Send,
{
    fn budget(&self) -> IterationBudget {
        IterationBudget {
            max_turns: self.agent.max_turns,
            token_budget: self.agent.token_budget,
            total_model_tokens: self.agent.total_model_tokens(),
        }
    }

    async fn request_model(&mut self, progress: LoopProgress) -> Result<ModelObservation> {
        let mailbox_messages = self.agent.inject_agent_mailbox_messages()?;
        if mailbox_messages > 0 {
            (self.report)(AgentEvent::Status(format!(
                "Delivered {mailbox_messages} agent mailbox message(s)."
            )));
        }
        self.agent.ensure_active_plan_step();
        self.agent.hook_output_candidates.clear();
        if let Some(runtime) = self.agent.hook_runtime.as_ref() {
            let outputs = runtime.blocking_drain_outputs();
            self.agent.hook_output_candidates = outputs
                .iter()
                .enumerate()
                .map(|(index, text)| hook_output_candidate(text, index, &self.agent.session_id))
                .collect();
            for text in outputs {
                self.agent.history.push(Message {
                    role: "system".to_string(),
                    content: Value::String(text),
                });
            }
        }
        let output = match self
            .agent
            .run_model_turn(self.output_mode, self.report)
            .await
        {
            Ok(output) => output,
            Err(error) if is_interrupt_error(&error) => {
                self.agent
                    .run_session_end_plugin_hooks(self.agent.latest_assistant_message_text(), true)
                    .await;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        self.agent
            .record_agent_turn_trace(&output, progress.agentic_turns, None, None, false);
        self.agent.last_query_plan_updated = output.plan_updated;
        if !output.tool_calls.is_empty() {
            let candidates: Vec<(String, String)> = output
                .tool_calls
                .iter()
                .map(|call| (call.name.clone(), call.input.to_string()))
                .collect();
            let repeated_previous = candidates
                .iter()
                .filter(|candidate| self.agent.recent_tool_calls.contains(candidate))
                .count();
            let repeated_adjacent = candidates.windows(2).any(|pair| pair[0] == pair[1]);
            self.agent.recent_tool_calls = candidates;
            if repeated_previous >= 2 || repeated_adjacent {
                (self.report)(AgentEvent::Status(
                    "Repeated tool call pattern detected. Consider re-evaluating the approach."
                        .to_string(),
                ));
            }
        }
        let observation = ModelObservation {
            tool_call_count: output.tool_calls.len(),
            plan_exit_call_id: output
                .tool_calls
                .iter()
                .find(|call| call.name == EXIT_PLAN_MODE_TOOL_NAME)
                .map(|call| call.id.clone()),
            plan_updated: output.plan_updated,
            malformed_proposed_plan: output.malformed_proposed_plan,
            continue_inspection: output.continue_inspection,
            response: ResponseEvidence {
                had_text_response: output.had_text_response,
                had_reasoning_response: output.had_reasoning_response,
            },
        };
        self.pending_turn = Some(LoopTurn {
            last_assistant_message: output.assistant_message.as_ref().and_then(message_text),
            output,
            assistant_message_recorded: false,
            stop_hook_block: None,
            tool_results: Vec::new(),
        });
        Ok(observation)
    }

    async fn record_assistant(&mut self, progress: LoopProgress) -> Result<ContinuationContext> {
        let turn = self
            .pending_turn
            .as_mut()
            .context("assistant effect without model output")?;
        turn.assistant_message_recorded = turn.output.assistant_message.is_some();
        if let Some(message) = turn.output.assistant_message.take() {
            self.agent.push_history_message(message);
            self.agent.checkpoint_session()?;
        }
        self.agent.record_agent_turn_trace(
            &turn.output,
            progress.agentic_turns,
            None,
            None,
            turn.assistant_message_recorded,
        );
        Ok(ContinuationContext {
            mode: self.agent.execution_mode,
            plan_steps: self.agent.current_plan.len(),
            inspection: InspectionEvidence {
                has_any_evidence: self.agent.inspection_progress.has_any_evidence(),
                has_minimum_review_evidence: self
                    .agent
                    .inspection_progress
                    .has_minimum_review_evidence(),
            },
            pending: PendingInteractions {
                user_input: self.agent.pending_user_input.is_some(),
                approval: self.agent.pending_approval.is_some(),
            },
        })
    }

    async fn continue_turn(
        &mut self,
        continuation: Continuation,
        progress: LoopProgress,
    ) -> Result<()> {
        let turn = self
            .pending_turn
            .as_ref()
            .context("continuation without model output")?;
        match continuation {
            Continuation::PlanExitRepair(rejection) => {
                (self.report)(rejected_plan_exit_event(&rejection));
                self.agent.record_agent_turn_trace(
                    &turn.output,
                    progress.agentic_turns,
                    Some("continued"),
                    Some(RuntimeContinuationPhase::PlanExitRepairRequired.label()),
                    false,
                );
                (self.report)(AgentEvent::Status(
                    "Plan exit was missing a structured proposed plan. Asking the model to repair the submission."
                        .to_string(),
                ));
                self.agent
                    .push_history_message(self.agent.runtime_continuation_message(
                        RuntimeContinuationPhase::PlanExitRepairRequired,
                        progress.agentic_turns,
                    ));
            }
            Continuation::Automatic(continuation) => {
                let (status, phase) = match continuation {
                    TextContinuation::PlanNeedsEvidence => (
                        "Plan mode needs more evidence. Continuing in read-only mode.",
                        RuntimeContinuationPhase::PlanContinuationRequired,
                    ),
                    TextContinuation::PlanReasoningOnly => (
                        "Plan mode needs more evidence. Continuing in read-only mode.",
                        RuntimeContinuationPhase::ReasoningOnlyContinuationRequired,
                    ),
                    TextContinuation::ExecuteNeedsInspection => (
                        "Repository review needs more code inspection. Continuing the same turn.",
                        RuntimeContinuationPhase::ExecutionContinuationRequired,
                    ),
                    TextContinuation::ExecuteReasoningOnly => (
                        "Model produced reasoning only. Continuing for a visible answer or tool call.",
                        RuntimeContinuationPhase::ReasoningOnlyContinuationRequired,
                    ),
                };
                (self.report)(AgentEvent::Status(status.to_string()));
                self.agent.record_agent_turn_trace(
                    &turn.output,
                    progress.agentic_turns,
                    Some("continued"),
                    Some(phase.label()),
                    turn.assistant_message_recorded,
                );
                self.agent.push_history_message(
                    self.agent
                        .runtime_continuation_message(phase, progress.agentic_turns),
                );
            }
            Continuation::StopHookBlocked => {
                let block = turn
                    .stop_hook_block
                    .as_ref()
                    .context("hook continuation without blocking output")?;
                (self.report)(AgentEvent::AgentError {
                    message: format!(
                        "Stop hook {} blocked completion: {}",
                        block.hook_id, block.reason
                    ),
                    recoverable: true,
                });
                (self.report)(AgentEvent::Status(
                    "Stop hook blocked completion. Continuing with hook feedback.".to_string(),
                ));
                self.agent.record_agent_turn_trace(
                    &turn.output,
                    progress.agentic_turns,
                    Some("continued"),
                    Some("stop_hook_blocked"),
                    turn.assistant_message_recorded,
                );
                self.agent.push_history_message(stop_hook_feedback(block));
            }
        }
        self.agent.checkpoint_session()?;
        Ok(())
    }

    async fn run_stop_hooks(
        &mut self,
        context: StopHookContext,
        _progress: LoopProgress,
    ) -> Result<StopHookOutcome> {
        let turn = self
            .pending_turn
            .as_mut()
            .context("Stop hook effect without model output")?;
        turn.stop_hook_block = self.agent.run_stop_hooks(
            turn.last_assistant_message.as_deref(),
            context.stop_hook_active,
            self.report,
        );
        let outcome = if turn.stop_hook_block.is_some() {
            StopHookOutcome::BlockCompletion
        } else {
            StopHookOutcome::AllowCompletion
        };
        Ok(outcome)
    }

    async fn run_tools(&mut self, progress: LoopProgress) -> Result<ToolBatchOutcome> {
        let turn = self
            .pending_turn
            .as_mut()
            .context("tool effect without model output")?;
        self.agent.record_agent_turn_trace(
            &turn.output,
            progress.agentic_turns,
            Some("running_tools"),
            Some("tool_calls_available"),
            turn.assistant_message_recorded,
        );
        let output = self
            .agent
            .execute_tool_calls(std::mem::take(&mut turn.output.tool_calls), self.report)
            .await?;
        turn.tool_results = output.messages;
        Ok(output.outcome)
    }

    async fn commit_tool_results(&mut self, progress: LoopProgress) -> Result<()> {
        let turn = self
            .pending_turn
            .as_mut()
            .context("tool result commit without model output")?;
        self.agent.advance_plan_step();
        self.agent.extend_history_for_next_turn(
            std::mem::take(&mut turn.tool_results),
            self.report,
            progress.agentic_turns,
        )?;
        Ok(())
    }

    async fn finalize(&mut self, end: LoopEnd, progress: LoopProgress) -> Result<()> {
        let last_assistant_message = match end {
            LoopEnd::MaxTurnsReached { limit } => {
                self.agent.last_agent_turn_trace.loop_outcome = Some("stopped".to_string());
                self.agent.last_agent_turn_trace.continuation_phase =
                    Some("max_turns_reached".to_string());
                (self.report)(AgentEvent::Status(format!(
                    "Agent reached max-turns limit ({limit})"
                )));
                self.agent.latest_assistant_message_text()
            }
            LoopEnd::TokenBudgetReached { budget, used } => {
                self.agent.token_budget_exhausted = true;
                self.agent.last_agent_turn_trace.loop_outcome = Some("stopped".to_string());
                self.agent.last_agent_turn_trace.continuation_phase =
                    Some("token_budget_exhausted".to_string());
                (self.report)(AgentEvent::Status(format!(
                    "Agent reached token budget ({used}/{budget})"
                )));
                self.agent.latest_assistant_message_text()
            }
            LoopEnd::PlanExitRepairExhausted(rejection) => {
                let turn = self
                    .pending_turn
                    .as_ref()
                    .context("plan repair failure without model output")?;
                (self.report)(rejected_plan_exit_event(&rejection));
                self.agent.record_agent_turn_trace(
                    &turn.output,
                    progress.agentic_turns,
                    Some("stopped"),
                    Some("plan_exit_repair_exhausted"),
                    false,
                );
                self.agent.checkpoint_session()?;
                self.agent.latest_assistant_message_text()
            }
            LoopEnd::ResponseComplete | LoopEnd::StopHookLimitReached { .. } => {
                let turn = self
                    .pending_turn
                    .as_ref()
                    .context("completion without model output")?;
                if let LoopEnd::StopHookLimitReached { limit } = end {
                    let block = turn
                        .stop_hook_block
                        .as_ref()
                        .context("hook limit without blocking output")?;
                    (self.report)(AgentEvent::AgentError {
                        message: format!(
                            "Stop hook {} continued to block after {limit} attempts; allowing completion.",
                            block.hook_id,
                        ),
                        recoverable: false,
                    });
                }
                self.agent.record_agent_turn_trace(
                    &turn.output,
                    progress.agentic_turns,
                    Some("stopped"),
                    Some("final_no_tool_response"),
                    turn.assistant_message_recorded,
                );
                self.agent.complete_active_plan_step();
                turn.last_assistant_message.clone()
            }
            LoopEnd::AwaitingApproval => {
                let turn = self
                    .pending_turn
                    .as_mut()
                    .context("approval pause without model output")?;
                // A partial batch must survive the pause without advancing the plan.
                self.agent
                    .extend_history_messages(std::mem::take(&mut turn.tool_results));
                self.agent.checkpoint_session()?;
                return Ok(());
            }
        };
        self.agent
            .run_session_end_plugin_hooks(last_assistant_message, false)
            .await;
        Ok(())
    }
}

fn rejected_plan_exit_event(rejection: &PlanExitRejection) -> AgentEvent {
    AgentEvent::ToolResult {
        call_id: rejection.call_id.clone(),
        name: EXIT_PLAN_MODE_TOOL_NAME.to_string(),
        content: match rejection.issue {
            PlanExitIssue::IncompletePlan => incomplete_proposed_plan_error(),
            PlanExitIssue::MissingPlan => missing_proposed_plan_error(),
        },
        is_error: true,
    }
}
