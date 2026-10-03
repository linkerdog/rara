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
        // Native approval helpers have appended their tool result and continuation.
        // Persist refreshed sources on that new context before the next model call.
        if self.persist_model_context_for_latest_user_message() {
            self.recompute_history_token_estimate();
            self.checkpoint_session()?;
        }
        let mut agentic_turns = 0usize;
        self.run_agent_loop_with_limit(output_mode, report, &mut agentic_turns)
            .await
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
        let reasoning_only = rara_agent::ResponseEvidence {
            had_text_response: turn_output.had_text_response,
            had_reasoning_response: turn_output.had_reasoning_response,
        }
        .is_reasoning_only();
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
    ) -> Result<rara_agent::ToolBatchOutput>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut output = rara_agent::execute_tool_batch(
            tool_calls,
            &mut super::tool_effects::NativeToolEffects {
                agent: self,
                report,
                entering_plan_mode: false,
            },
        )
        .await?;
        if self.pending_approval.is_some() || self.pending_plan_exit_tool_id.is_some() {
            output.outcome = rara_agent::ToolBatchOutcome::AwaitingApproval;
        }
        output.messages = enforce_tool_result_batch_budget(output.messages);
        Ok(output)
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
        let call = self
            .inference_context
            .as_ref()
            .map(|context| context.start_call(rara_observability::InferencePurpose::Classifier));
        let mut metadata = self.llm_turn_metadata();
        if let Some(call) = &call {
            metadata = metadata.with_inference(call.context());
        }
        let raw = self
            .llm_backend
            .classify_with_context(instructions, &messages, metadata)
            .await;
        if let Some(call) = call {
            call.finish(&raw);
        }
        let raw = raw?;
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
        if let Some(inference) = &self.inference_context {
            context = context.with_inference(inference.clone());
        }
        match self.cancellation_token.as_ref() {
            Some(token) => context.with_cancellation(token.clone()),
            None => context,
        }
    }
}
