use super::*;

impl Agent {
    pub(super) async fn run_agent_loop<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut agentic_turns = 0usize;
        self.run_agent_loop_with_limit(output_mode, report, &mut agentic_turns)
            .await
    }

    /// Post-turn consolidation check (Claude Code style).
    ///
    /// Checks whether memory consolidation is due.  When sessions are
    /// ready, acquires the lock and dispatches a Consolidate subagent
    pub(super) async fn run_agent_loop_with_limit<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
        agentic_turns: &mut usize,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut plan_exit_repair_attempts = 0usize;
        let mut stop_hook_continuations = 0usize;
        let mut session_end_last_assistant_message: Option<String> = None;
        let mut should_run_session_end_hooks = false;
        loop {
            if let Some(max) = self.max_turns
                && *agentic_turns >= max
            {
                self.last_agent_turn_trace.loop_outcome = Some("stopped".to_string());
                self.last_agent_turn_trace.continuation_phase =
                    Some("max_turns_reached".to_string());
                report(AgentEvent::Status(format!(
                    "Agent reached max-turns limit ({max})",
                )));
                session_end_last_assistant_message = self.latest_assistant_message_text();
                should_run_session_end_hooks = true;
                break;
            }
            if let Some(budget) = self.token_budget
                && self.total_model_tokens() >= budget
            {
                self.token_budget_exhausted = true;
                self.last_agent_turn_trace.loop_outcome = Some("stopped".to_string());
                self.last_agent_turn_trace.continuation_phase =
                    Some("token_budget_exhausted".to_string());
                report(AgentEvent::Status(format!(
                    "Agent reached token budget ({}/{budget})",
                    self.total_model_tokens()
                )));
                session_end_last_assistant_message = self.latest_assistant_message_text();
                should_run_session_end_hooks = true;
                break;
            }
            let mailbox_messages = self.inject_agent_mailbox_messages()?;
            if mailbox_messages > 0 {
                report(AgentEvent::Status(format!(
                    "Delivered {mailbox_messages} agent mailbox message(s)."
                )));
            }
            self.ensure_active_plan_step();
            // Inject hook outputs as system messages before the model turn
            self.hook_output_candidates.clear();
            if let Some(hr) = self.hook_runtime.as_ref() {
                let outputs = hr.blocking_drain_outputs();
                self.hook_output_candidates = outputs
                    .iter()
                    .enumerate()
                    .map(|(index, text)| hook_output_candidate(text, index, &self.session_id))
                    .collect();
                for text in outputs {
                    self.history.push(Message {
                        role: "system".to_string(),
                        content: Value::String(text),
                    });
                }
            }
            let mut turn_output = match self.run_model_turn(output_mode, report).await {
                Ok(turn_output) => turn_output,
                Err(err) if is_interrupt_error(&err) => {
                    self.run_session_end_plugin_hooks(self.latest_assistant_message_text(), true)
                        .await;
                    return Err(err);
                }
                Err(err) => return Err(err),
            };
            self.record_agent_turn_trace(&turn_output, *agentic_turns, None, None, false);
            self.last_query_plan_updated = turn_output.plan_updated;
            if !turn_output.tool_calls.is_empty() {
                // Detect repeated tool calls — both within a single turn
                // and across consecutive turns.
                let mut identical_calls_within_turn = 0;
                let candidates: Vec<(String, String)> = turn_output
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        let input_key = serde_json::to_string(&tc.input).unwrap_or_default();
                        (tc.name.clone(), input_key)
                    })
                    .collect();
                let prev = &self.recent_tool_calls;
                let dup_count = candidates.iter().filter(|c| prev.contains(c)).count();
                // Also check within-turn repeats
                if candidates.len() >= 2 {
                    for i in 1..candidates.len() {
                        if candidates[i] == candidates[i - 1] {
                            identical_calls_within_turn += 1;
                        }
                    }
                }
                self.recent_tool_calls = candidates;
                if dup_count >= 2 || identical_calls_within_turn >= 1 {
                    report(AgentEvent::Status(
                        "Repeated tool call pattern detected. Consider re-evaluating the approach."
                            .to_string(),
                    ));
                }
            }
            if turn_output
                .tool_calls
                .iter()
                .any(|tool_call| tool_call.name == EXIT_PLAN_MODE_TOOL_NAME)
                && (turn_output.malformed_proposed_plan || !turn_output.plan_updated)
            {
                let content = if turn_output.malformed_proposed_plan {
                    incomplete_proposed_plan_error()
                } else {
                    missing_proposed_plan_error()
                };
                report(AgentEvent::ToolResult {
                    call_id: turn_output
                        .tool_calls
                        .iter()
                        .find(|tool_call| tool_call.name == EXIT_PLAN_MODE_TOOL_NAME)
                        .expect("exit plan mode call checked above")
                        .id
                        .clone(),
                    name: EXIT_PLAN_MODE_TOOL_NAME.to_string(),
                    content: content.clone(),
                    is_error: true,
                });
                if plan_exit_repair_attempts < MAX_PLAN_EXIT_REPAIR_ATTEMPTS {
                    plan_exit_repair_attempts += 1;
                    *agentic_turns += 1;
                    self.record_agent_turn_trace(
                        &turn_output,
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
                    self.checkpoint_session()?;
                    continue;
                }
                self.record_agent_turn_trace(
                    &turn_output,
                    *agentic_turns,
                    Some("stopped"),
                    Some("plan_exit_repair_exhausted"),
                    false,
                );
                self.checkpoint_session()?;
                session_end_last_assistant_message = self.latest_assistant_message_text();
                should_run_session_end_hooks = true;
                break;
            }
            let last_assistant_message = turn_output
                .assistant_message
                .as_ref()
                .and_then(message_text);
            let assistant_message_recorded = turn_output.assistant_message.is_some();
            if let Some(message) = turn_output.assistant_message.take() {
                self.push_history_message(message);
                self.checkpoint_session()?;
            }
            self.record_agent_turn_trace(
                &turn_output,
                *agentic_turns,
                None,
                None,
                assistant_message_recorded,
            );

            if turn_output.tool_calls.is_empty() {
                let is_reasoning_only = Self::is_reasoning_only_turn(
                    turn_output.had_text_response,
                    turn_output.had_reasoning_response,
                );
                if self.should_continue_plan_without_tools(
                    turn_output.plan_updated,
                    turn_output.continue_inspection,
                    turn_output.had_text_response,
                    turn_output.had_reasoning_response,
                    *agentic_turns,
                ) {
                    report(AgentEvent::Status(
                        "Plan mode needs more evidence. Continuing in read-only mode.".to_string(),
                    ));
                    *agentic_turns += 1;
                    let phase = if is_reasoning_only {
                        RuntimeContinuationPhase::ReasoningOnlyContinuationRequired
                    } else {
                        RuntimeContinuationPhase::PlanContinuationRequired
                    };
                    self.record_agent_turn_trace(
                        &turn_output,
                        *agentic_turns,
                        Some("continued"),
                        Some(phase.label()),
                        assistant_message_recorded,
                    );
                    self.push_history_message(
                        self.runtime_continuation_message(phase, *agentic_turns),
                    );
                    self.checkpoint_session()?;
                    continue;
                }
                if self.should_continue_execute_without_tools(
                    turn_output.continue_inspection,
                    turn_output.had_text_response,
                    turn_output.had_reasoning_response,
                ) {
                    let phase = if is_reasoning_only {
                        report(AgentEvent::Status(
                            "Model produced reasoning only. Continuing for a visible answer or tool call."
                                .to_string(),
                        ));
                        RuntimeContinuationPhase::ReasoningOnlyContinuationRequired
                    } else {
                        report(AgentEvent::Status(
                            "Repository review needs more code inspection. Continuing the same turn."
                                .to_string(),
                        ));
                        RuntimeContinuationPhase::ExecutionContinuationRequired
                    };
                    *agentic_turns += 1;
                    self.record_agent_turn_trace(
                        &turn_output,
                        *agentic_turns,
                        Some("continued"),
                        Some(phase.label()),
                        assistant_message_recorded,
                    );
                    self.push_history_message(
                        self.runtime_continuation_message(phase, *agentic_turns),
                    );
                    self.checkpoint_session()?;
                    continue;
                }
                if let Some(block) = self.run_stop_hooks(
                    last_assistant_message.as_deref(),
                    stop_hook_continuations > 0,
                    report,
                ) {
                    if stop_hook_continuations < MAX_STOP_HOOK_CONTINUATIONS {
                        stop_hook_continuations += 1;
                        *agentic_turns += 1;
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
                            &turn_output,
                            *agentic_turns,
                            Some("continued"),
                            Some("stop_hook_blocked"),
                            assistant_message_recorded,
                        );
                        self.push_history_message(stop_hook_feedback(&block));
                        self.checkpoint_session()?;
                        continue;
                    }
                    report(AgentEvent::AgentError {
                        message: format!(
                            "Stop hook {} continued to block after {MAX_STOP_HOOK_CONTINUATIONS} attempts; allowing completion.",
                            block.hook_id
                        ),
                        recoverable: false,
                    });
                }
                self.record_agent_turn_trace(
                    &turn_output,
                    *agentic_turns,
                    Some("stopped"),
                    Some("final_no_tool_response"),
                    assistant_message_recorded,
                );
                self.complete_active_plan_step();
                session_end_last_assistant_message = last_assistant_message;
                should_run_session_end_hooks = true;
                break;
            }
            *agentic_turns += 1;
            self.record_agent_turn_trace(
                &turn_output,
                *agentic_turns,
                Some("running_tools"),
                Some("tool_calls_available"),
                assistant_message_recorded,
            );

            let tool_results = self
                .execute_tool_calls(turn_output.tool_calls, report)
                .await?;
            if self.pending_approval.is_some() || self.pending_plan_exit_tool_id.is_some() {
                self.checkpoint_session()?;
                break;
            }
            self.advance_plan_step();
            self.extend_history_for_next_turn(tool_results, report, *agentic_turns)?;
        }
        if should_run_session_end_hooks {
            self.run_session_end_plugin_hooks(session_end_last_assistant_message, false)
                .await;
        }
        Ok(())
    }

    pub(super) async fn run_session_end_plugin_hooks(
        &self,
        last_assistant_message: Option<String>,
        is_interrupt: bool,
    ) {
        if let Some(plugin_hooks) = self.plugin_hook_runtime.clone() {
            plugin_hooks
                .run_session_end(last_assistant_message.as_deref(), is_interrupt)
                .await;
        }
    }

    pub(super) async fn run_plugin_session_start_hooks_once(&mut self) {
        if self.plugin_session_start_hooks_ran {
            return;
        }
        if let Some(plugin_hooks) = self.plugin_hook_runtime.clone() {
            self.plugin_session_start_hooks_ran = true;
            plugin_hooks.run_session_start().await;
        }
    }

    pub(super) async fn run_user_prompt_submit_plugin_hooks(&self, prompt: &str) {
        if let Some(plugin_hooks) = self.plugin_hook_runtime.clone() {
            plugin_hooks.run_user_prompt_submit(prompt).await;
        }
    }

    pub(super) fn latest_assistant_message_text(&self) -> Option<String> {
        self.history
            .iter()
            .rev()
            .find(|message| message.role == "assistant")
            .and_then(message_text)
    }

    pub(super) fn run_stop_hooks<F>(
        &self,
        last_assistant_message: Option<&str>,
        stop_hook_active: bool,
        report: &mut F,
    ) -> Option<StopHookBlock>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let (Some(registry), Some(sandbox)) = (&self.hook_registry, &self.hook_sandbox) else {
            return None;
        };
        let input = json!({
            "session_id": self.session_id,
            "cwd": sandbox.workspace_root,
            "hook_event_name": "Stop",
            "stop_hook_active": stop_hook_active,
            "last_assistant_message": last_assistant_message.unwrap_or_default(),
        })
        .to_string();

        for hook in registry.executable_hooks_for_phase(HookLifecycle::Stop) {
            match run_sandboxed_hook(hook, sandbox, &input) {
                Ok(outcome) => {
                    if outcome.timed_out {
                        report(AgentEvent::Status(format!(
                            "Stop hook {} timed out; allowing completion.",
                            hook.id
                        )));
                        continue;
                    }
                    if let Some(reason) = stop_hook_block_reason(&outcome) {
                        return Some(StopHookBlock {
                            hook_id: hook.id.clone(),
                            reason,
                        });
                    }
                    if outcome.exit_code.is_some_and(|code| code != 0) {
                        report(AgentEvent::Status(format!(
                            "Stop hook {} exited unsuccessfully; allowing completion: {}",
                            hook.id,
                            outcome.stderr.trim()
                        )));
                    }
                }
                Err(error) => report(AgentEvent::Status(format!(
                    "Stop hook {} failed; allowing completion: {error}",
                    hook.id
                ))),
            }
        }
        None
    }

    pub(super) fn record_agent_turn_trace(
        &mut self,
        turn_output: &TurnOutput,
        agentic_turn_index: usize,
        loop_outcome: Option<&str>,
        continuation_phase: Option<&str>,
        assistant_message_recorded: bool,
    ) {
        let reasoning_only = Self::is_reasoning_only_turn(
            turn_output.had_text_response,
            turn_output.had_reasoning_response,
        );
        self.last_agent_turn_trace = AgentTurnTraceView {
            agentic_turn_index,
            execution_mode: self.execution_mode_label().to_string(),
            model_stop_reason: turn_output.model_stop_reason.clone(),
            loop_outcome: loop_outcome.map(ToString::to_string),
            continuation_phase: continuation_phase.map(ToString::to_string),
            had_text_response: turn_output.had_text_response,
            had_reasoning_response: turn_output.had_reasoning_response,
            reasoning_only,
            streamed_text_delta: turn_output.streamed_text_delta,
            streamed_reasoning_delta: turn_output.streamed_reasoning_delta,
            assistant_message_recorded,
            tool_call_count: turn_output.tool_calls.len(),
            plan_updated: turn_output.plan_updated,
            continue_inspection: turn_output.continue_inspection,
            malformed_proposed_plan: turn_output.malformed_proposed_plan,
        };
    }

    pub(super) async fn try_continue_after_recoverable_runtime_error<F>(
        &mut self,
        err: &anyhow::Error,
        output_mode: AgentOutputMode,
        report: &mut F,
        agentic_turns: &mut usize,
        runtime_error_recoveries: &mut usize,
    ) -> Result<bool>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let Some(kind) = recoverable_runtime_error_kind(err) else {
            return Ok(false);
        };
        if *runtime_error_recoveries >= MAX_RUNTIME_ERROR_RECOVERY_ATTEMPTS {
            return Ok(false);
        }
        *runtime_error_recoveries += 1;
        report(AgentEvent::Status(format!(
            "Recoverable local runtime error detected ({kind}). Asking the model to handle it."
        )));
        self.push_history_message(recoverable_runtime_error_message(kind, err));
        self.run_agent_loop_with_limit(output_mode, report, agentic_turns)
            .await?;
        Ok(true)
    }

    pub(super) async fn execute_tool_calls<F>(
        &mut self,
        tool_calls: Vec<ToolCall>,
        report: &mut F,
    ) -> Result<Vec<Message>>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut tool_results = Vec::new();
        let entering_plan_mode = tool_calls
            .iter()
            .any(|tool_call| tool_call.name == ENTER_PLAN_MODE_TOOL_NAME);
        if entering_plan_mode && !matches!(self.execution_mode, AgentExecutionMode::Plan) {
            self.execution_mode = AgentExecutionMode::Plan;
            report(AgentEvent::Status(
                "Entered read-only planning mode.".to_string(),
            ));
        }
        for tool_call in tool_calls {
            let tool_name = tool_call.name.clone();
            let tool_id = tool_call.id.clone();
            let tool_input = tool_call.input.clone();
            if tool_name == ENTER_PLAN_MODE_TOOL_NAME {
                let result_text = json!({
                    "status": "entered_plan_mode",
                    "instructions": [
                        "Inspect the repository with read-only tools.",
                        "Return a normal final answer for research, review, or planning-advice tasks.",
                        "Use a <proposed_plan> block only when you are requesting approval to implement a concrete plan.",
                        "Call exit_plan_mode only after the same assistant message contains a complete <proposed_plan>...</proposed_plan> block.",
                        "Use <request_user_input> only when a blocking decision needs user input.",
                        "Use <continue_inspection/> only when another read-only inspection pass is required."
                    ]
                })
                .to_string();
                report(AgentEvent::ToolResult {
                    call_id: tool_id.clone(),
                    name: tool_name,
                    content: result_text.clone(),
                    is_error: false,
                });
                tool_results.push(tool_result_message(&tool_id, result_text, false));
                continue;
            }
            if tool_name == EXIT_PLAN_MODE_TOOL_NAME {
                if self.current_plan.is_empty() {
                    let error_text = missing_proposed_plan_error();
                    report(AgentEvent::ToolResult {
                        call_id: tool_id.clone(),
                        name: tool_name.clone(),
                        content: error_text.clone(),
                        is_error: true,
                    });
                    tool_results.push(tool_result_message(&tool_id, error_text, true));
                    continue;
                }
                self.pending_plan_exit_tool_id = Some(tool_id);
                report(AgentEvent::ApprovalRequested {
                    approval_id: self
                        .pending_plan_exit_tool_id
                        .clone()
                        .expect("plan approval id was just assigned"),
                    kind: "plan".to_string(),
                });
                report(AgentEvent::Status(
                    "Plan ready for approval. Waiting for a structured user decision.".to_string(),
                ));
                break;
            }
            let bash_request = if tool_call.name == "bash" {
                match BashCommandInput::from_value(tool_call.input.clone()) {
                    Ok(request) => Some(request),
                    Err(err) => {
                        let error_text = format!("Error: invalid bash payload: {err}");
                        report(AgentEvent::ToolResult {
                            call_id: tool_id.clone(),
                            name: tool_name.clone(),
                            content: error_text.clone(),
                            is_error: true,
                        });
                        tool_results.push(tool_result_message(&tool_id, error_text, true));
                        continue;
                    }
                }
            } else {
                None
            };
            if let Some(request) = bash_request.as_ref()
                && matches!(self.execution_mode, AgentExecutionMode::Plan)
                && !request.is_read_only()
            {
                let error_text = format!(
                    "Error: bash is read-only in plan mode. Refuse command '{}' and inspect with read-only commands or return a plan.",
                    request.summary()
                );
                report(AgentEvent::ToolResult {
                    call_id: tool_id.clone(),
                    name: tool_name.clone(),
                    content: error_text.clone(),
                    is_error: true,
                });
                tool_results.push(tool_result_message(&tool_id, error_text, true));
                continue;
            }
            if let Some(request) = bash_request.as_ref()
                && !self.full_access_mode
                && (request.requires_escalated_permissions()
                    || matches!(self.bash_approval_mode, BashApprovalMode::Suggestion))
            {
                if request.is_read_only() || self.is_bash_prefix_approved(request) {
                    report(AgentEvent::Status(format!(
                        "Shell command allowed by policy: {}",
                        request.summary()
                    )));
                } else {
                    self.pending_approval = Some(PendingApproval {
                        tool_use_id: tool_id.clone(),
                        request: request.to_owned(),
                    });
                    report(AgentEvent::ApprovalRequested {
                        approval_id: tool_id.clone(),
                        kind: "shell".to_string(),
                    });
                    report(AgentEvent::Status(
                        "Bash approval required. Waiting for a structured user decision."
                            .to_string(),
                    ));
                    break;
                }
            }
            // ── Auto-permission classifier safety net ────────────────────────────
            // Safety net: for dangerous tools (bash, web_*, pty), run the LLM
            // classifier to detect suspicious commands the static rules missed.
            // Explicit full access delegates that boundary to the caller's
            // external isolation and therefore bypasses this local gate.
            const CLASSIFIABLE_TOOLS: &[&str] =
                &["bash", "pty", "web_search", "web_fetch", "mcp_tool_search"];
            if !self.full_access_mode && CLASSIFIABLE_TOOLS.contains(&tool_name.as_str()) {
                let classifier_input = tool_input.clone();
                let request = crate::classifier::AutoPermissionRequest {
                    tool_name: tool_name.clone(),
                    tool_input: classifier_input,
                    workspace_hint: Some(self.workspace.root.display().to_string()),
                };
                match self.classify_auto_permission(&request).await {
                    Ok(resp) => {
                        report(AgentEvent::Status(format!(
                            "Auto-permission: {} — {}",
                            resp.decision, resp.reason,
                        )));
                        match resp.decision {
                            crate::classifier::AutoPermissionDecision::Deny => {
                                let error_text = format!(
                                    "Error: auto-permission classifier denied this tool call: {}",
                                    resp.reason
                                );
                                report(AgentEvent::ToolResult {
                                    call_id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    content: error_text.clone(),
                                    is_error: true,
                                });
                                tool_results.push(tool_result_message(&tool_id, error_text, true));
                                continue;
                            }
                            crate::classifier::AutoPermissionDecision::Allow
                            | crate::classifier::AutoPermissionDecision::Ask => {
                                // Allow — proceed to existing checks
                            }
                        }
                    }
                    Err(e) => {
                        // Classifier unavailable — fail open (existing checks remain)
                        report(AgentEvent::Status(format!(
                            "Auto-permission classifier unavailable: {e}"
                        )));
                    }
                }
            }
            // ── end auto-permission classifier ───────────────────────────────────

            if !self.is_tool_allowed_in_current_mode(&tool_name) {
                let error_text = format!(
                    "Error: tool '{}' is unavailable in {} mode. Inspect with read-only tools and return a plan instead.",
                    tool_name,
                    self.execution_mode_label()
                );
                report(AgentEvent::ToolResult {
                    call_id: tool_id.clone(),
                    name: tool_name.clone(),
                    content: error_text.clone(),
                    is_error: true,
                });
                tool_results.push(tool_result_message(&tool_id, error_text, true));
                continue;
            }
            // PreToolUse hook: run registered hooks that can allow/block.
            if let (Some(registry), Some(sandbox)) = (&self.hook_registry, &self.hook_sandbox) {
                let hooks = registry.executable_hooks_for_phase(HookLifecycle::PreToolUse);
                let mut blocked = false;
                if !hooks.is_empty() {
                    let input = serde_json::json!({
                        "tool_name": tool_name,
                        "tool_input": tool_input
                    });
                    let input_str = input.to_string();
                    for hook in &hooks {
                        match run_sandboxed_hook(hook, sandbox, &input_str) {
                            Ok(outcome) if !outcome.allows() => {
                                let msg = format!("tool {} blocked by hook {}", tool_name, hook.id);
                                tool_results.push(tool_result_message(&tool_id, msg, true));
                                if !outcome.stderr.is_empty() {
                                    eprintln!("hook {}: {}", hook.id, outcome.stderr);
                                }
                                blocked = true;
                                break;
                            }
                            Err(e) => {
                                eprintln!("hook {} failed: {}", hook.id, e);
                                blocked = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                if blocked {
                    continue;
                }
            }
            if let Some(plugin_hooks) = self.plugin_hook_runtime.clone()
                && let Some(block) = plugin_hooks.run_pre_tool_use(&tool_name, &tool_input).await
            {
                let error_text = format!(
                    "Error: tool {} blocked by plugin hook {}: {}",
                    tool_name, block.plugin_name, block.message
                );
                report(AgentEvent::ToolResult {
                    call_id: tool_id.clone(),
                    name: tool_name.clone(),
                    content: error_text.clone(),
                    is_error: true,
                });
                tool_results.push(tool_result_message(&tool_id, error_text, true));
                continue;
            }
            if let Some(tool) = self.tool_manager.get_tool(&tool_name) {
                self.inspection_progress
                    .record_tool(&tool_name, &tool_input);
                let status_detail = if tool_name == "bash" {
                    BashCommandInput::from_value(tool_input.clone())
                        .map(|request| format!("Running shell command: {}", request.summary()))
                        .unwrap_or_else(|_| "Running shell command.".to_string())
                } else {
                    format!("Running tool {}.", tool_name)
                };
                report(AgentEvent::Status(status_detail));
                match tool
                    .call_with_context_events(
                        tool_input.clone(),
                        self.tool_call_context(&tool_id),
                        &mut |progress| match progress {
                            ToolProgressEvent::Output { stream, chunk } => {
                                report(AgentEvent::ToolProgress {
                                    call_id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    stream,
                                    chunk,
                                });
                            }
                        },
                    )
                    .await
                {
                    Ok(result) => {
                        if tool_name == TODO_WRITE_TOOL_NAME {
                            let state: TodoState = serde_json::from_value(result.clone())?;
                            if let Err(err) = self
                                .session_manager
                                .save_todo_state(&self.session_id, &state)
                            {
                                report(AgentEvent::Status(format!(
                                    "Warning: failed to persist todo state: {err}"
                                )));
                            }
                            self.todo_state = Some(state.clone());
                            report(AgentEvent::TodoUpdated(state));
                        }
                        // Accumulate subagent (auxiliary model) cache statistics.
                        if matches!(
                            tool_name.as_str(),
                            "spawn_agent" | "explore_agent" | "plan_agent" | "team_create"
                        ) {
                            let (hit, miss) = if tool_name == "team_create" {
                                // team_create nests results under "team_results[*]".
                                result["team_results"]
                                    .as_array()
                                    .map(|results| {
                                        results.iter().fold((0, 0), |(h, m), res| {
                                            (
                                                h + res["cache_hit_tokens"].as_u64().unwrap_or(0)
                                                    as u32,
                                                m + res["cache_miss_tokens"].as_u64().unwrap_or(0)
                                                    as u32,
                                            )
                                        })
                                    })
                                    .unwrap_or((0, 0))
                            } else {
                                (
                                    result["cache_hit_tokens"].as_u64().unwrap_or(0) as u32,
                                    result["cache_miss_tokens"].as_u64().unwrap_or(0) as u32,
                                )
                            };
                            self.accumulate_aux_cache(hit, miss);
                        }
                        let result_text = self.tool_result_store.compact_result(
                            &tool_name,
                            &tool_id,
                            &tool_input,
                            &result,
                        )?;
                        report(AgentEvent::ToolResult {
                            call_id: tool_id.clone(),
                            name: tool_name.clone(),
                            content: result_text.clone(),
                            is_error: false,
                        });
                        tool_results.push(tool_result_message(&tool_id, result_text, false));
                    }
                    Err(e) => {
                        let error_text = format!("Error: {}", e);
                        report(AgentEvent::ToolResult {
                            call_id: tool_id.clone(),
                            name: tool_name.clone(),
                            content: error_text.clone(),
                            is_error: true,
                        });
                        tool_results.push(tool_result_message(&tool_id, error_text, true));
                    }
                }
            }
        }
        Ok(enforce_tool_result_batch_budget(tool_results))
    }

    /// Classify whether a tool call should be auto-allowed, denied, or requires
    /// user approval. Delegates to the LLM backend's auxiliary model.
    pub(super) async fn classify_auto_permission(
        &self,
        request: &crate::classifier::AutoPermissionRequest,
    ) -> Result<crate::classifier::AutoPermissionResponse> {
        let instructions = "\
You are a security classifier. Given a user message and a proposed tool call,
output exactly one JSON object with fields:
- \"decision\": \"allow\", \"deny\", or \"ask\"
- \"reason\": a short justification
- \"matched_rule\": optional policy rule name

Rules:
- allow: read-only, safe filesystem operations within the workspace, standard build/test/lint/format commands, git status/diff/log
- deny: destructive commands (rm -rf, format disk), privilege escalation (sudo), modifying system files outside workspace, accessing sensitive paths (/etc/passwd)
- ask: network requests (curl, web_fetch), git push/commit, installing packages, modifying configs outside workspace, commands with unclear intent
        ";

        let messages = crate::classifier::build_classifier_messages(
            &self.history,
            &request.tool_name,
            &request.tool_input,
        );
        let raw = self.llm_backend.classify(instructions, &messages).await?;
        Ok(crate::classifier::parse_auto_permission_response(&raw)?)
    }

    pub(super) fn tool_call_context(&self, call_id: &str) -> ToolCallContext {
        let mut context = ToolCallContext::default()
            .with_session_id(self.session_id.clone())
            .with_call_id(call_id)
            .with_workspace_root(self.workspace.root.clone());
        if let Some(turn_id) = &self.runtime_turn_id {
            context = context.with_turn_id(turn_id.clone());
        }
        match self.cancellation_token.as_ref() {
            Some(token) => context.with_cancellation(token.clone()),
            None => context,
        }
    }
}
