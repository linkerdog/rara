use rara_agent::{ToolAdmission, ToolBatchEffects, ToolReply, execute_tool_call};
use rara_core::tool::ToolError;

use super::*;

pub(super) struct NativeToolEffects<'a, F> {
    pub(super) agent: &'a mut Agent,
    pub(super) report: &'a mut F,
    pub(super) entering_plan_mode: bool,
}

#[async_trait::async_trait]
impl<F: FnMut(AgentEvent) + Send> ToolBatchEffects for NativeToolEffects<'_, F> {
    async fn begin_batch(&mut self, calls: &[ToolCall]) -> Result<()> {
        self.entering_plan_mode = calls
            .iter()
            .any(|tool_call| tool_call.name == ENTER_PLAN_MODE_TOOL_NAME)
            && self
                .agent
                .is_tool_allowed_in_current_mode(ENTER_PLAN_MODE_TOOL_NAME);
        if self.entering_plan_mode && !matches!(self.agent.execution_mode, AgentExecutionMode::Plan)
        {
            self.agent.execution_mode = AgentExecutionMode::Plan;
            (self.report)(AgentEvent::Status(
                "Entered read-only planning mode.".to_string(),
            ));
        }
        Ok(())
    }

    async fn prepare_call(&mut self, tool_call: &ToolCall) -> Result<ToolAdmission> {
        let tool_name = tool_call.name.clone();
        let tool_id = tool_call.id.clone();
        let tool_input = tool_call.input.clone();
        if let Some(context) = &self.agent.inference_context {
            context.record_tool_request();
        }
        if !(self.agent.is_tool_allowed_in_current_mode(&tool_name)
            || (tool_name == ENTER_PLAN_MODE_TOOL_NAME && self.entering_plan_mode))
        {
            if let Some(context) = &self.agent.inference_context {
                context.record_tool_rejection();
            }
            let error_text = format!(
                "Error: tool '{}' is unavailable in {} mode. Inspect with read-only tools and return a plan instead.",
                tool_name,
                self.agent.execution_mode_label()
            );
            (self.report)(AgentEvent::ToolResult {
                call_id: tool_id.clone(),
                name: tool_name.clone(),
                content: error_text.clone(),
                is_error: true,
            });
            return Ok(ToolAdmission::Reply(ToolReply::error(error_text)));
        }
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
            (self.report)(AgentEvent::ToolResult {
                call_id: tool_id.clone(),
                name: tool_name,
                content: result_text.clone(),
                is_error: false,
            });
            return Ok(ToolAdmission::Reply(ToolReply::success(result_text)));
        }
        if tool_name == EXIT_PLAN_MODE_TOOL_NAME {
            if self.agent.current_plan.is_empty() {
                let error_text = missing_proposed_plan_error();
                (self.report)(AgentEvent::ToolResult {
                    call_id: tool_id.clone(),
                    name: tool_name.clone(),
                    content: error_text.clone(),
                    is_error: true,
                });
                return Ok(ToolAdmission::Reply(ToolReply::error(error_text)));
            }
            self.agent.pending_plan_exit_tool_id = Some(tool_id.clone());
            (self.report)(AgentEvent::ApprovalRequested {
                approval_id: tool_id,
                kind: "plan".to_string(),
            });
            (self.report)(AgentEvent::Status(
                "Plan ready for approval. Waiting for a structured user decision.".to_string(),
            ));
            return Ok(ToolAdmission::AwaitingApproval);
        }
        let bash_request = if tool_call.name == "bash" {
            match BashCommandInput::from_value(tool_call.input.clone()) {
                Ok(request) => Some(request),
                Err(err) => {
                    let error_text = format!("Error: invalid bash payload: {err}");
                    (self.report)(AgentEvent::ToolResult {
                        call_id: tool_id.clone(),
                        name: tool_name.clone(),
                        content: error_text.clone(),
                        is_error: true,
                    });
                    return Ok(ToolAdmission::Reply(ToolReply::error(error_text)));
                }
            }
        } else {
            None
        };
        if let Some(request) = bash_request.as_ref()
            && matches!(self.agent.execution_mode, AgentExecutionMode::Plan)
            && !request.is_read_only()
        {
            let error_text = format!(
                "Error: bash is read-only in plan mode. Refuse command '{}' and inspect with read-only commands or return a plan.",
                request.summary()
            );
            (self.report)(AgentEvent::ToolResult {
                call_id: tool_id.clone(),
                name: tool_name.clone(),
                content: error_text.clone(),
                is_error: true,
            });
            return Ok(ToolAdmission::Reply(ToolReply::error(error_text)));
        }
        if let Some(request) = bash_request.as_ref()
            && !self.agent.full_access_mode
            && (request.requires_escalated_permissions()
                || matches!(self.agent.bash_approval_mode, BashApprovalMode::Suggestion))
        {
            if request.is_read_only() || self.agent.is_bash_prefix_approved(request) {
                (self.report)(AgentEvent::Status(format!(
                    "Shell command allowed by policy: {}",
                    request.summary()
                )));
            } else {
                self.agent.pending_approval = Some(PendingApproval {
                    tool_use_id: tool_id.clone(),
                    request: request.to_owned(),
                });
                (self.report)(AgentEvent::ApprovalRequested {
                    approval_id: tool_id.clone(),
                    kind: "shell".to_string(),
                });
                (self.report)(AgentEvent::Status(
                    "Bash approval required. Waiting for a structured user decision.".to_string(),
                ));
                return Ok(ToolAdmission::AwaitingApproval);
            }
        }
        // ── Auto-permission classifier safety net ────────────────────────────
        // Safety net: for dangerous tools (bash, web_*, pty), run the LLM
        // classifier to detect suspicious commands the static rules missed.
        // Explicit full access delegates that boundary to the caller's
        // external isolation and therefore bypasses this local gate.
        const CLASSIFIABLE_TOOLS: &[&str] =
            &["bash", "pty", "web_search", "web_fetch", "mcp_tool_search"];
        if !self.agent.full_access_mode && CLASSIFIABLE_TOOLS.contains(&tool_name.as_str()) {
            let classifier_input = tool_input.clone();
            let request = crate::classifier::AutoPermissionRequest {
                tool_name: tool_name.clone(),
                tool_input: classifier_input,
                workspace_hint: Some(self.agent.workspace.root.display().to_string()),
            };
            match self.agent.classify_auto_permission(&request).await {
                Ok(resp) => {
                    (self.report)(AgentEvent::Status(format!(
                        "Auto-permission: {} — {}",
                        resp.decision, resp.reason,
                    )));
                    match resp.decision {
                        crate::classifier::AutoPermissionDecision::Deny => {
                            let error_text = format!(
                                "Error: auto-permission classifier denied this tool call: {}",
                                resp.reason
                            );
                            (self.report)(AgentEvent::ToolResult {
                                call_id: tool_id.clone(),
                                name: tool_name.clone(),
                                content: error_text.clone(),
                                is_error: true,
                            });
                            return Ok(ToolAdmission::Reply(ToolReply::error(error_text)));
                        }
                        crate::classifier::AutoPermissionDecision::Allow
                        | crate::classifier::AutoPermissionDecision::Ask => {
                            // Allow — proceed to existing checks
                        }
                    }
                }
                Err(e) => {
                    // Classifier unavailable — fail open (existing checks remain)
                    (self.report)(AgentEvent::Status(format!(
                        "Auto-permission classifier unavailable: {e}"
                    )));
                }
            }
        }
        // ── end auto-permission classifier ───────────────────────────────────

        if let (Some(registry), Some(sandbox)) =
            (&self.agent.hook_registry, &self.agent.hook_sandbox)
        {
            let hooks = registry.executable_hooks_for_phase(HookLifecycle::PreToolUse);
            if !hooks.is_empty() {
                let input = json!({"tool_name": tool_name, "tool_input": tool_input}).to_string();
                for hook in &hooks {
                    match run_sandboxed_hook(hook, sandbox, &input) {
                        Ok(outcome) if !outcome.allows() => {
                            let message = format!("tool {} blocked by hook {}", tool_name, hook.id);
                            if !outcome.stderr.is_empty() {
                                log::warn!("hook {}: {}", hook.id, outcome.stderr);
                            }
                            return Ok(ToolAdmission::Reply(ToolReply::error(message)));
                        }
                        Err(error) => {
                            log::warn!("hook {} failed: {}", hook.id, error);
                            return Ok(ToolAdmission::Omit);
                        }
                        Ok(_) => {}
                    }
                }
            }
        }
        if let Some(plugin_hooks) = self.agent.plugin_hook_runtime.clone()
            && let Some(block) = plugin_hooks.run_pre_tool_use(&tool_name, &tool_input).await
        {
            let error_text = format!(
                "Error: tool {} blocked by plugin hook {}: {}",
                tool_name, block.plugin_name, block.message
            );
            (self.report)(AgentEvent::ToolResult {
                call_id: tool_id.clone(),
                name: tool_name.clone(),
                content: error_text.clone(),
                is_error: true,
            });
            return Ok(ToolAdmission::Reply(ToolReply::error(error_text)));
        }
        if self.agent.tool_manager.get_tool(&tool_name).is_none() {
            if let Some(context) = &self.agent.inference_context {
                context.record_tool_rejection();
            }
            let error_text =
                format!("Error: tool '{tool_name}' is not registered in this session.");
            (self.report)(AgentEvent::ToolResult {
                call_id: tool_id,
                name: tool_name,
                content: error_text.clone(),
                is_error: true,
            });
            return Ok(ToolAdmission::Reply(ToolReply::error(error_text)));
        }
        self.agent
            .inspection_progress
            .record_tool(&tool_name, &tool_input);
        let status_detail = if tool_name == "bash" {
            BashCommandInput::from_value(tool_input.clone())
                .map(|request| format!("Running shell command: {}", request.summary()))
                .unwrap_or_else(|_| "Running shell command.".to_string())
        } else {
            format!("Running tool {}.", tool_name)
        };
        (self.report)(AgentEvent::Status(status_detail));
        Ok(ToolAdmission::Invoke)
    }

    async fn invoke_call(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        let tool = self
            .agent
            .tool_manager
            .get_tool(&call.name)
            .ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "tool '{}' is not registered in this session",
                    call.name
                ))
            })?;
        execute_tool_call(
            tool,
            call,
            self.agent.tool_call_context(&call.id),
            &mut |progress| match progress.event {
                ToolProgressEvent::Output { stream, chunk } => {
                    (self.report)(AgentEvent::ToolProgress {
                        call_id: progress.call_id,
                        name: progress.name,
                        stream,
                        chunk,
                    })
                }
            },
        )
        .await
    }

    async fn complete_call(
        &mut self,
        call: &ToolCall,
        result: Result<Value, ToolError>,
    ) -> Result<ToolReply> {
        let tool_name = call.name.clone();
        let tool_id = call.id.clone();
        let tool_input = call.input.clone();
        match result {
            Ok(result) => {
                if tool_name == TODO_WRITE_TOOL_NAME {
                    let state: TodoState = serde_json::from_value(result.clone())?;
                    if let Err(err) = self
                        .agent
                        .session_manager
                        .save_todo_state(&self.agent.session_id, &state)
                    {
                        (self.report)(AgentEvent::Status(format!(
                            "Warning: failed to persist todo state: {err}"
                        )));
                    }
                    self.agent.todo_state = Some(state.clone());
                    (self.report)(AgentEvent::TodoUpdated(state));
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
                                        h + res["cache_hit_tokens"].as_u64().unwrap_or(0) as u32,
                                        m + res["cache_miss_tokens"].as_u64().unwrap_or(0) as u32,
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
                    self.agent.accumulate_aux_cache(hit, miss);
                }
                let result_text = self.agent.tool_result_store.compact_result(
                    &tool_name,
                    &tool_id,
                    &tool_input,
                    &result,
                )?;
                (self.report)(AgentEvent::ToolResult {
                    call_id: tool_id.clone(),
                    name: tool_name.clone(),
                    content: result_text.clone(),
                    is_error: false,
                });
                Ok(ToolReply::success(result_text))
            }
            Err(e) => {
                let error_text = format!("Error: {}", e);
                (self.report)(AgentEvent::ToolResult {
                    call_id: tool_id.clone(),
                    name: tool_name.clone(),
                    content: error_text.clone(),
                    is_error: true,
                });
                Ok(ToolReply::error(error_text))
            }
        }
    }
}
