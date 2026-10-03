use rara_agent::{
    Continuation, ContinuationContext, InspectionEvidence, IterationBudget, LoopEffect, LoopEnd,
    LoopMachine, LoopProgress, LoopRequest, ModelObservation, PendingInteractions, PlanExitIssue,
    PlanExitRejection, ResponseEvidence, StopHookOutcome, TextContinuation, ToolBatchOutcome,
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
    /// Execute host effects while the portable machine owns loop transitions.
    pub(super) async fn run_agent_loop_with_limit<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
        agentic_turns: &mut usize,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut machine = LoopMachine::new(LoopProgress {
            agentic_turns: *agentic_turns,
        });
        let mut request = machine.begin_iteration(self.loop_budget())?;
        let mut pending_turn: Option<LoopTurn> = None;
        loop {
            // Keep outer runtime-error recovery's progress even when an effect fails.
            *agentic_turns = machine.progress().agentic_turns;
            let LoopRequest { id, effect } = request;
            request = match effect {
                LoopEffect::RequestModel => {
                    let mailbox_messages = self.inject_agent_mailbox_messages()?;
                    if mailbox_messages > 0 {
                        report(AgentEvent::Status(format!(
                            "Delivered {mailbox_messages} agent mailbox message(s)."
                        )));
                    }
                    self.ensure_active_plan_step();
                    self.hook_output_candidates.clear();
                    if let Some(runtime) = self.hook_runtime.as_ref() {
                        let outputs = runtime.blocking_drain_outputs();
                        self.hook_output_candidates = outputs
                            .iter()
                            .enumerate()
                            .map(|(index, text)| {
                                hook_output_candidate(text, index, &self.session_id)
                            })
                            .collect();
                        for text in outputs {
                            self.history.push(Message {
                                role: "system".to_string(),
                                content: Value::String(text),
                            });
                        }
                    }
                    let output = match self.run_model_turn(output_mode, report).await {
                        Ok(output) => output,
                        Err(error) if is_interrupt_error(&error) => {
                            self.run_session_end_plugin_hooks(
                                self.latest_assistant_message_text(),
                                true,
                            )
                            .await;
                            return Err(error);
                        }
                        Err(error) => return Err(error),
                    };
                    self.record_agent_turn_trace(&output, *agentic_turns, None, None, false);
                    self.last_query_plan_updated = output.plan_updated;
                    if !output.tool_calls.is_empty() {
                        let candidates: Vec<(String, String)> = output
                            .tool_calls
                            .iter()
                            .map(|call| (call.name.clone(), call.input.to_string()))
                            .collect();
                        let repeated_previous = candidates
                            .iter()
                            .filter(|candidate| self.recent_tool_calls.contains(candidate))
                            .count();
                        let repeated_adjacent =
                            candidates.windows(2).any(|pair| pair[0] == pair[1]);
                        self.recent_tool_calls = candidates;
                        if repeated_previous >= 2 || repeated_adjacent {
                            report(AgentEvent::Status(
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
                    pending_turn = Some(LoopTurn {
                        last_assistant_message: output
                            .assistant_message
                            .as_ref()
                            .and_then(message_text),
                        output,
                        assistant_message_recorded: false,
                        stop_hook_block: None,
                        tool_results: Vec::new(),
                    });
                    machine.model_completed(id, observation)?
                }
                LoopEffect::RecordAssistant => {
                    let turn = pending_turn
                        .as_mut()
                        .context("assistant effect without model output")?;
                    turn.assistant_message_recorded = turn.output.assistant_message.is_some();
                    if let Some(message) = turn.output.assistant_message.take() {
                        self.push_history_message(message);
                        self.checkpoint_session()?;
                    }
                    self.record_agent_turn_trace(
                        &turn.output,
                        *agentic_turns,
                        None,
                        None,
                        turn.assistant_message_recorded,
                    );
                    machine.assistant_recorded(
                        id,
                        ContinuationContext {
                            mode: self.execution_mode,
                            plan_steps: self.current_plan.len(),
                            inspection: InspectionEvidence {
                                has_any_evidence: self.inspection_progress.has_any_evidence(),
                                has_minimum_review_evidence: self
                                    .inspection_progress
                                    .has_minimum_review_evidence(),
                            },
                            pending: PendingInteractions {
                                user_input: self.pending_user_input.is_some(),
                                approval: self.pending_approval.is_some(),
                            },
                        },
                    )?
                }
                LoopEffect::Continue(continuation) => {
                    let turn = pending_turn
                        .as_ref()
                        .context("continuation without model output")?;
                    match continuation {
                        Continuation::PlanExitRepair(rejection) => {
                            report(rejected_plan_exit_event(&rejection));
                            self.record_agent_turn_trace(
                                &turn.output,
                                *agentic_turns,
                                Some("continued"),
                                Some(RuntimeContinuationPhase::PlanExitRepairRequired.label()),
                                false,
                            );
                            report(AgentEvent::Status(
                                "Plan exit was missing a structured proposed plan. Asking the model to repair the submission."
                                    .to_string(),
                            ));
                            self.push_history_message(self.runtime_continuation_message(
                                RuntimeContinuationPhase::PlanExitRepairRequired,
                                *agentic_turns,
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
                            report(AgentEvent::Status(status.to_string()));
                            self.record_agent_turn_trace(
                                &turn.output,
                                *agentic_turns,
                                Some("continued"),
                                Some(phase.label()),
                                turn.assistant_message_recorded,
                            );
                            self.push_history_message(
                                self.runtime_continuation_message(phase, *agentic_turns),
                            );
                        }
                        Continuation::StopHookBlocked => {
                            let block = turn
                                .stop_hook_block
                                .as_ref()
                                .context("hook continuation without blocking output")?;
                            report(AgentEvent::AgentError {
                                message: format!(
                                    "Stop hook {} blocked completion: {}",
                                    block.hook_id, block.reason
                                ),
                                recoverable: true,
                            });
                            report(AgentEvent::Status(
                                "Stop hook blocked completion. Continuing with hook feedback."
                                    .to_string(),
                            ));
                            self.record_agent_turn_trace(
                                &turn.output,
                                *agentic_turns,
                                Some("continued"),
                                Some("stop_hook_blocked"),
                                turn.assistant_message_recorded,
                            );
                            self.push_history_message(stop_hook_feedback(block));
                        }
                    }
                    self.checkpoint_session()?;
                    machine.checkpoint_completed(id, self.loop_budget())?
                }
                LoopEffect::RunStopHooks { stop_hook_active } => {
                    let turn = pending_turn
                        .as_mut()
                        .context("Stop hook effect without model output")?;
                    turn.stop_hook_block = self.run_stop_hooks(
                        turn.last_assistant_message.as_deref(),
                        stop_hook_active,
                        report,
                    );
                    let outcome = if turn.stop_hook_block.is_some() {
                        StopHookOutcome::BlockCompletion
                    } else {
                        StopHookOutcome::AllowCompletion
                    };
                    machine.stop_hooks_completed(id, outcome)?
                }
                LoopEffect::RunTools => {
                    let turn = pending_turn
                        .as_mut()
                        .context("tool effect without model output")?;
                    self.record_agent_turn_trace(
                        &turn.output,
                        *agentic_turns,
                        Some("running_tools"),
                        Some("tool_calls_available"),
                        turn.assistant_message_recorded,
                    );
                    turn.tool_results = self
                        .execute_tool_calls(std::mem::take(&mut turn.output.tool_calls), report)
                        .await?;
                    let outcome = if self.pending_approval.is_some()
                        || self.pending_plan_exit_tool_id.is_some()
                    {
                        ToolBatchOutcome::AwaitingApproval
                    } else {
                        ToolBatchOutcome::ResultsAvailable
                    };
                    machine.tools_completed(id, outcome)?
                }
                LoopEffect::CommitToolResults => {
                    let turn = pending_turn
                        .as_mut()
                        .context("tool result commit without model output")?;
                    self.advance_plan_step();
                    self.extend_history_for_next_turn(
                        std::mem::take(&mut turn.tool_results),
                        report,
                        *agentic_turns,
                    )?;
                    machine.checkpoint_completed(id, self.loop_budget())?
                }
                LoopEffect::Finalize(end) => {
                    let last_assistant_message = match end {
                        LoopEnd::MaxTurnsReached { limit } => {
                            self.last_agent_turn_trace.loop_outcome = Some("stopped".to_string());
                            self.last_agent_turn_trace.continuation_phase =
                                Some("max_turns_reached".to_string());
                            report(AgentEvent::Status(format!(
                                "Agent reached max-turns limit ({limit})"
                            )));
                            self.latest_assistant_message_text()
                        }
                        LoopEnd::TokenBudgetReached { budget, used } => {
                            self.token_budget_exhausted = true;
                            self.last_agent_turn_trace.loop_outcome = Some("stopped".to_string());
                            self.last_agent_turn_trace.continuation_phase =
                                Some("token_budget_exhausted".to_string());
                            report(AgentEvent::Status(format!(
                                "Agent reached token budget ({used}/{budget})"
                            )));
                            self.latest_assistant_message_text()
                        }
                        LoopEnd::PlanExitRepairExhausted(rejection) => {
                            let turn = pending_turn
                                .as_ref()
                                .context("plan repair failure without model output")?;
                            report(rejected_plan_exit_event(&rejection));
                            self.record_agent_turn_trace(
                                &turn.output,
                                *agentic_turns,
                                Some("stopped"),
                                Some("plan_exit_repair_exhausted"),
                                false,
                            );
                            self.checkpoint_session()?;
                            self.latest_assistant_message_text()
                        }
                        LoopEnd::ResponseComplete | LoopEnd::StopHookLimitReached { .. } => {
                            let turn = pending_turn
                                .as_ref()
                                .context("completion without model output")?;
                            if let LoopEnd::StopHookLimitReached { limit } = end {
                                let block = turn
                                    .stop_hook_block
                                    .as_ref()
                                    .context("hook limit without blocking output")?;
                                report(AgentEvent::AgentError {
                                    message: format!(
                                        "Stop hook {} continued to block after {limit} attempts; allowing completion.",
                                        block.hook_id,
                                    ),
                                    recoverable: false,
                                });
                            }
                            self.record_agent_turn_trace(
                                &turn.output,
                                *agentic_turns,
                                Some("stopped"),
                                Some("final_no_tool_response"),
                                turn.assistant_message_recorded,
                            );
                            self.complete_active_plan_step();
                            turn.last_assistant_message.clone()
                        }
                        LoopEnd::AwaitingApproval => {
                            self.checkpoint_session()?;
                            machine.finalization_completed(id)?;
                            return Ok(());
                        }
                    };
                    self.run_session_end_plugin_hooks(last_assistant_message, false)
                        .await;
                    machine.finalization_completed(id)?;
                    return Ok(());
                }
            };
        }
    }

    fn loop_budget(&self) -> IterationBudget {
        IterationBudget {
            max_turns: self.max_turns,
            token_budget: self.token_budget,
            total_model_tokens: self.total_model_tokens(),
        }
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
