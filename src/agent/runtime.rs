use super::*;

impl Agent {
    /// Configure hook execution context. Called after construction.
    pub fn set_hook_context(
        &mut self,
        registry: Arc<crate::hooks::HookRegistry>,
        sandbox: HookSandbox,
        runtime: Arc<HookRuntime>,
    ) {
        self.hook_registry = Some(registry);
        self.hook_sandbox = Some(sandbox);
        self.hook_runtime = Some(runtime);
    }

    /// Configure Claude plugin hooks loaded for this runtime session.
    pub(crate) fn set_plugin_hook_runtime(
        &mut self,
        runtime: Arc<crate::plugin_middleware::PluginHookRuntime>,
    ) {
        self.plugin_hook_runtime = Some(runtime);
    }

    pub(crate) fn plugin_command_count(&self) -> usize {
        self.plugin_hook_runtime
            .as_ref()
            .map_or(0, |runtime| runtime.command_summaries().len())
    }

    /// Accumulate subagent (auxiliary model) cache statistics.
    /// Called by consolidation and other subagent completion
    /// handlers to split cache reporting between main and aux models.
    pub fn accumulate_aux_cache(&mut self, hit: u32, miss: u32) {
        self.aux_total_cache_hit_tokens += hit;
        self.aux_total_cache_miss_tokens += miss;
    }

    #[cfg(test)]
    pub fn new(
        tool_manager: ToolManager,
        llm_backend: Arc<dyn LlmBackend>,
        memory_handle: Arc<MemoryHandle>,
        session_manager: Arc<SessionManager>,
        workspace: Arc<WorkspaceMemory>,
    ) -> Self {
        let agent_definitions = AgentDefinitionCache::load(workspace.root.clone());
        Self::new_with_agent_definitions(
            tool_manager,
            llm_backend,
            memory_handle,
            session_manager,
            workspace,
            agent_definitions,
        )
    }

    pub fn new_with_agent_definitions(
        tool_manager: ToolManager,
        llm_backend: Arc<dyn LlmBackend>,
        memory_handle: Arc<MemoryHandle>,
        session_manager: Arc<SessionManager>,
        workspace: Arc<WorkspaceMemory>,
        agent_definitions: AgentDefinitionCache,
    ) -> Self {
        let root = workspace.root.clone();
        let memory_store = Arc::new(MemoryStore::new_with_handle(
            llm_backend.clone(),
            memory_handle.clone(),
        ));
        let state_db =
            session_manager.storage_dir.parent().and_then(
                |rara_dir| match StateDb::new_for_root_dir(rara_dir.to_path_buf()) {
                    Ok(state_db) => Some(Arc::new(state_db)),
                    Err(err) => {
                        eprintln!(
                            "Warning: could not initialize session state db at {}: {err}",
                            rara_dir.display()
                        );
                        None
                    }
                },
            );
        let memory_root = if let Some(rara_dir) = session_manager.storage_dir.parent() {
            rara_dir.join("memory")
        } else {
            workspace.root.join(".rara").join("memory")
        };
        let consolidation_config = rara_memory::consolidation::ConsolidationConfig::default();
        let consolidation_scheduler = rara_memory::consolidation::ConsolidationScheduler::new(
            memory_root,
            consolidation_config,
        );
        Self {
            tool_manager,
            llm_backend,
            memory_handle,
            memory_store,
            session_manager,
            consolidation_scheduler,
            state_db,
            workspace,
            history: Vec::new(),
            session_id: Uuid::new_v4().to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_hit_tokens: 0,
            total_cache_miss_tokens: 0,
            aux_total_cache_hit_tokens: 0,
            aux_total_cache_miss_tokens: 0,
            tool_result_store: ToolResultStore::new(
                default_tool_result_store_dir().unwrap_or_else(|_| {
                    std::env::temp_dir().join(format!("rara-tool-results-{}", Uuid::new_v4()))
                }),
            )
            .unwrap_or_else(|err| {
                eprintln!("Warning: could not create tool result store: {err}");
                ToolResultStore::new(std::env::temp_dir().join("rara-fallback")).unwrap_or_else(
                    |_| {
                        // Absolute last resort: use a /tmp subdir that should always work
                        ToolResultStore::new(format!("/tmp/rara-tool-results-{}", Uuid::new_v4()))
                            .expect("unrecoverable: cannot create tool result store")
                    },
                )
            }),
            execution_mode: AgentExecutionMode::Execute,
            max_turns: None,
            token_budget: None,
            token_budget_exhausted: false,
            bash_approval_mode: BashApprovalMode::Always,
            full_access_mode: false,
            current_plan: Vec::new(),
            plan_explanation: None,
            pending_user_input: None,
            pending_approval: None,
            todo_state: None,
            task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
            agent_definitions,
            completed_user_input: None,
            completed_approval: None,
            approved_bash_prefixes: Vec::new(),
            compact_state: CompactState::default(),
            hook_registry: None,
            hook_sandbox: None,
            hook_runtime: None,
            plugin_hook_runtime: None,
            plugin_session_start_hooks_ran: false,
            retrieved_memory_candidates: Vec::new(),
            file_search_candidates: Vec::new(),
            mcp_resource_candidates: Vec::new(),
            hook_output_candidates: Vec::new(),
            graph_context_candidates: Vec::new(),
            last_tool_result_projection_report: ToolResultProjectionReport::default(),
            last_agent_turn_trace: AgentTurnTraceView::default(),
            file_search_provider: FileSearchCandidateProvider::new(root, true),
            inspection_progress: InspectionProgress::default(),
            last_query_plan_updated: false,
            recent_tool_calls: Vec::new(),
            pending_plan_exit_tool_id: None,
            prompt_config: PromptRuntimeConfig::default(),
            prompt_source_registry: None,
            skill_source_registry: None,
            lsp_manager: None,
            agent_tree_control: None,
            cancellation_token: None,
            last_interaction_time: std::time::Instant::now(),
        }
    }

    pub async fn query(&mut self, prompt: String) -> Result<()> {
        self.query_with_mode(prompt, AgentOutputMode::Terminal)
            .await
    }

    pub(crate) fn set_agent_tree_control(
        &mut self,
        control: Option<Arc<crate::tools::agent::AgentTreeControl>>,
    ) {
        self.agent_tree_control = control;
    }

    pub fn agent_tree_control(&self) -> Option<Arc<crate::tools::agent::AgentTreeControl>> {
        self.agent_tree_control.clone()
    }

    pub fn agent_definition_records(&self) -> Vec<AgentDefinitionLoadRecord> {
        self.agent_definitions.records()
    }

    pub async fn query_with_mode(
        &mut self,
        prompt: String,
        output_mode: AgentOutputMode,
    ) -> Result<()> {
        self.query_with_mode_and_events(prompt, output_mode, |_| {})
            .await
    }

    pub async fn query_with_mode_and_events<F>(
        &mut self,
        prompt: String,
        output_mode: AgentOutputMode,
        mut report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let turn_start_idx = self.history.len();
        self.last_interaction_time = std::time::Instant::now();
        let mut agentic_turns = 0usize;
        let mut runtime_error_recoveries = 0usize;
        self.inspection_progress = InspectionProgress::default();
        self.last_query_plan_updated = false;
        self.pending_plan_exit_tool_id = None;
        self.run_plugin_session_start_hooks_once().await;
        self.run_user_prompt_submit_plugin_hooks(&prompt).await;
        self.compact_if_needed_with_reporter(&mut report).await?;
        let repaired_history = repair_tool_result_history(&self.history);
        if repaired_history != self.history {
            self.replace_history(repaired_history);
            self.checkpoint_session()?;
        }
        self.clear_completed_interactions();

        self.push_history_message(Message {
            role: "user".to_string(),
            content: json!([{"type": "text", "text": prompt.clone()}]),
        });
        self.checkpoint_session()?;
        report(AgentEvent::MemoryAction {
            message: memory_notice("querying workspace memory"),
        });
        self.refresh_memory_retrieval_candidates().await;
        report(AgentEvent::MemoryAction {
            message: memory_notice(
                self.workspace
                    .memory_notice_text(self.retrieved_memory_candidates.len()),
            ),
        });
        self.refresh_file_search_candidates();
        self.refresh_protocol_prompt_sources_for_query().await;
        self.refresh_protocol_skill_sources_for_query().await;

        match self
            .run_agent_loop_with_limit(output_mode, &mut report, &mut agentic_turns)
            .await
        {
            Ok(()) => {
                // Post-turn consolidation check (fire-and-forget).
                let sessions = self.consolidation_scheduler.check();
                if sessions.is_some() {
                    let prompt_config = self.prompt_config.clone();
                    let llm_backend = self.llm_backend.clone();
                    let memory_handle = self.memory_handle.clone();
                    let session_manager = self.session_manager.clone();
                    let workspace = self.workspace.clone();
                    let scheduler = self.consolidation_scheduler.clone();
                    let task_list_id = self.task_list_id.clone();
                    let agent_definitions = self.agent_definitions.clone();
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("consolidation runtime");
                        rt.block_on(async move {
                            let Some(sessions) = sessions else { return };
                            let Some(_lock) = scheduler.acquire_lock() else {
                                return;
                            };
                            let prompt =
                                rara_memory::dream_prompts::build_consolidation_prompt(&sessions);
                            eprintln!(
                                "consolidation: {} sessions ready, dispatching subagent",
                                sessions.len()
                            );
                            let result = crate::tools::agent::run_sub_agent(
                                crate::tools::agent::SubAgentKind::Consolidate,
                                &uuid::Uuid::new_v4().to_string(),
                                None,
                                Some("consolidation"),
                                None,
                                &prompt,
                                None,
                                None,
                                None,
                                llm_backend,
                                Arc::new(crate::tools::agent::InheritedSubagentBackendResolver),
                                memory_handle,
                                session_manager,
                                workspace,
                                prompt_config,
                                task_list_id,
                                agent_definitions,
                                None,
                                None,
                            )
                            .await;
                            match result {
                                Ok(r) => {
                                    let line = if r.summary.is_empty() {
                                        format!(
                                            "📝 consolidation complete — status={} (cache: {}/{} hit/miss)",
                                            r.status, r.total_cache_hit_tokens, r.total_cache_miss_tokens
                                        )
                                    } else {
                                        format!(
                                            "📝 consolidation: {} (cache: {}/{} hit/miss)",
                                            r.summary, r.total_cache_hit_tokens, r.total_cache_miss_tokens
                                        )
                                    };
                                    eprintln!("{}", line);
                                }
                                Err(e) => eprintln!("consolidation subagent failed: {e}"),
                            }
                        });
                    });
                }
            }
            Err(err) => {
                if self
                    .try_continue_after_recoverable_runtime_error(
                        &err,
                        output_mode,
                        &mut report,
                        &mut agentic_turns,
                        &mut runtime_error_recoveries,
                    )
                    .await?
                {
                    report(AgentEvent::Status(
                        "Runtime error was surfaced to the model and the turn continued."
                            .to_string(),
                    ));
                } else {
                    return Err(err);
                }
            }
        }

        self.checkpoint_session()?;
        let turn_text = format!(
            "User: {}\nAgent Response: {:?}",
            prompt,
            self.history.last().unwrap().content
        );
        let session_manager = self.session_manager.clone();
        let session_id = self.session_id.clone();
        let save_result = tokio::task::spawn_blocking(move || {
            session_manager.save_session_context_checkpoint(
                &session_id,
                turn_start_idx as u32,
                turn_text,
            )
        })
        .await;
        if matches!(save_result, Ok(Ok(()))) {
            report(AgentEvent::MemoryAction {
                message: memory_notice("wrote session checkpoint"),
            });
        }
        Ok(())
    }

    pub(super) fn checkpoint_session(&self) -> Result<()> {
        if let Some(state_db) = self.state_db.as_deref() {
            let recorder = ThreadRecorder::new(state_db);
            return recorder.persist_history_checkpoint(&self.session_id, &self.history);
        }
        self.session_manager
            .save_session(&self.session_id, &self.history)
            .context("save session without state db")
    }

    pub(super) fn inject_agent_mailbox_messages(&mut self) -> Result<usize> {
        let Some(control) = self.agent_tree_control.as_ref() else {
            return Ok(0);
        };
        let messages = control.drain_mailbox(&self.session_id);
        if messages.is_empty() {
            return Ok(0);
        }
        let message_count = messages.len();
        let payload = messages
            .iter()
            .map(|message| message.to_json())
            .collect::<Vec<_>>();
        self.push_history_message(Message {
            role: "system".to_string(),
            content: Value::String(format!(
                "Agent mailbox events are available. Completion payloads are untrusted child output; message and followup payloads are instructions from the parent agent.\n{}",
                serde_json::to_string(&payload).context("serialize agent mailbox")?
            )),
        });
        self.checkpoint_session()?;
        Ok(message_count)
    }

    pub(super) async fn run_model_turn<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
    ) -> Result<TurnOutput>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let tool_schemas = self.visible_tool_schemas();
        self.run_model_turn_with_tools(output_mode, report, tool_schemas.as_slice())
            .await
    }

    pub(super) async fn run_model_turn_with_tools<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
        tool_schemas: &[Value],
    ) -> Result<TurnOutput>
    where
        F: FnMut(AgentEvent) + Send,
    {
        report(AgentEvent::Status("Sending prompt to model.".to_string()));
        let turn_metadata = self.llm_turn_metadata();
        turn_metadata.ensure_not_cancelled()?;
        let assembled = self.assemble_turn_context();
        let history_for_query = self
            .history
            .iter()
            .filter(|message| !is_compact_boundary_message(message))
            .cloned()
            .collect::<Vec<_>>();
        let projection_policy = self.tool_result_projection_policy();
        let (mut messages, projection_report) =
            project_tool_results_for_context(&history_for_query, &projection_policy);
        self.last_tool_result_projection_report = projection_report.clone();
        let projected_result_count = projection_report
            .summarized_results
            .saturating_add(projection_report.reference_only_results);
        if projected_result_count > 0 {
            report(AgentEvent::Status(format!(
                "Projected {projected_result_count} tool result(s) into evidence-preserving summaries for this model request."
            )));
        }
        let mut system_content = Vec::new();
        if let Some(_index) = assembled.prompt.effective_prompt.dynamic_boundary_index {
            let full_text = &assembled.prompt.effective_prompt.text;
            // The text is joined by "\n\n". We want to split it back or just use the sections.
            // But EffectivePrompt only gives us the full text and boundary index.
            // Actually, build_effective_prompt joins them.

            let parts: Vec<&str> = full_text
                .split(rara_instructions::DYNAMIC_BOUNDARY)
                .collect();
            if parts.len() >= 2 {
                let static_part = parts[0].trim();
                let dynamic_part = parts[1..].join(rara_instructions::DYNAMIC_BOUNDARY);
                let dynamic_part = dynamic_part.trim();

                if !static_part.is_empty() {
                    system_content.push(json!({
                        "type": "text",
                        "text": static_part,
                        "cache_control": {"type": "ephemeral"} // Add hint for Anthropic-style caching
                    }));
                }
                // Add the boundary itself if needed or just skip it.
                // Claude Code keeps it to mark the boundary for future edits.
                system_content.push(json!({
                    "type": "text",
                    "text": rara_instructions::DYNAMIC_BOUNDARY,
                }));
                if !dynamic_part.is_empty() {
                    system_content.push(json!({
                        "type": "text",
                        "text": dynamic_part,
                    }));
                }
            } else {
                system_content.push(json!(assembled.prompt.effective_prompt.text));
            }
        } else {
            system_content.push(json!(assembled.prompt.effective_prompt.text));
        }

        messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: if system_content.len() == 1 {
                    system_content.remove(0)
                } else {
                    Value::Array(system_content)
                },
            },
        );
        if let Some(memory_context) = Agent::selected_memory_context_text(&assembled.runtime) {
            Agent::prepend_memory_context_to_latest_user_message(&mut messages, memory_context);
        }

        let model_label = self.model_event_label();
        report(AgentEvent::ModelRequest {
            model: model_label.clone(),
            // Provider token usage is only available after the response.
            // RuntimeControl documents 0 here as the unknown-count sentinel.
            input_tokens: 0,
        });

        let mut streamed_any_text_delta = false;
        let mut streamed_any_reasoning_delta = false;
        let response = self
            .llm_backend
            .ask_streaming_with_context(&messages, tool_schemas, turn_metadata, &mut |event| {
                match event {
                    LlmStreamEvent::TextDelta(delta) => {
                        streamed_any_text_delta = true;
                        report(AgentEvent::AssistantDelta(delta));
                    }
                    LlmStreamEvent::ReasoningDelta(delta) => {
                        streamed_any_reasoning_delta = true;
                        report(AgentEvent::AssistantThinkingDelta(delta));
                    }
                }
            })
            .await?;

        let output_tokens = response
            .usage
            .as_ref()
            .map(|usage| usage.output_tokens)
            .unwrap_or(0);
        report(AgentEvent::ModelResponse {
            model: model_label,
            output_tokens,
            finish_reason: response.stop_reason.clone(),
        });

        if let Some(usage) = &response.usage {
            self.total_input_tokens += usage.input_tokens;
            self.total_output_tokens += usage.output_tokens;
            self.total_cache_hit_tokens += usage.cache_hit_tokens;
            self.total_cache_miss_tokens += usage.cache_miss_tokens;
        }

        let mut tool_calls = Vec::new();
        let mut plan_updated = false;
        let mut malformed_proposed_plan = false;
        let mut continue_inspection = false;
        let mut had_text_response = false;
        let mut had_reasoning_response = streamed_any_reasoning_delta;
        let mut sanitized_content = Vec::new();
        for block in &response.content {
            match block {
                ContentBlock::Text { text } => {
                    let (clean_text, block_requests_continue) =
                        planning::strip_continue_inspection_control(text);
                    continue_inspection |= block_requests_continue;
                    let clean_text = scrub_internal_control_tokens(&clean_text);
                    if !clean_text.trim().is_empty() {
                        had_text_response = true;
                        sanitized_content.push(ContentBlock::Text {
                            text: clean_text.clone(),
                        });
                        if !streamed_any_text_delta {
                            report(AgentEvent::AssistantText(clean_text.clone()));
                        }
                        if matches!(self.execution_mode, AgentExecutionMode::Plan) {
                            malformed_proposed_plan |=
                                planning::has_unclosed_proposed_plan_block(&clean_text);
                            if self.capture_plan_from_text(&clean_text)? {
                                plan_updated = true;
                                report(AgentEvent::PlanUpdated {
                                    steps: self.current_plan.clone(),
                                    explanation: self.plan_explanation.clone(),
                                });
                            }
                        }
                        if matches!(output_mode, AgentOutputMode::Terminal) {
                            println!("Agent: {}", clean_text);
                        }
                    }
                }
                ContentBlock::ToolUse { id, name, input } => {
                    if matches!(self.execution_mode, AgentExecutionMode::Plan)
                        && name == EXIT_PLAN_MODE_TOOL_NAME
                        && !plan_updated
                        && let Some((steps, explanation)) =
                            planning::parse_exit_plan_tool_input(input)
                    {
                        self.current_plan = steps;
                        self.plan_explanation = explanation;
                        plan_updated = true;
                        report(AgentEvent::PlanUpdated {
                            steps: self.current_plan.clone(),
                            explanation: self.plan_explanation.clone(),
                        });
                    }
                    sanitized_content.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                    let modified_input = match self.hook_runtime.as_ref() {
                        Some(runtime) => runtime.modify_tool_input(name.as_str(), input.clone()),
                        None => input.clone(),
                    };
                    report(AgentEvent::ToolUse {
                        name: name.clone(),
                        input: modified_input.clone(),
                    });
                    tool_calls.push(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: modified_input,
                    });
                }
                ContentBlock::ProviderMetadata {
                    provider,
                    key,
                    value,
                } => {
                    sanitized_content.push(ContentBlock::ProviderMetadata {
                        provider: provider.clone(),
                        key: key.clone(),
                        value: value.clone(),
                    });
                    if key == "reasoning_content"
                        && value.as_str().is_some_and(|text| !text.trim().is_empty())
                    {
                        had_reasoning_response = true;
                    }
                }
            }
        }
        if matches!(self.execution_mode, AgentExecutionMode::Plan) && plan_updated {
            self.save_current_plan_file()?;
        }

        Ok(TurnOutput {
            assistant_message: assistant_turn_history_message(sanitized_content)?,
            tool_calls,
            plan_updated,
            malformed_proposed_plan,
            continue_inspection,
            had_text_response,
            had_reasoning_response,
            streamed_text_delta: streamed_any_text_delta,
            streamed_reasoning_delta: streamed_any_reasoning_delta,
            model_stop_reason: response.stop_reason,
        })
    }

    pub(super) fn llm_turn_metadata(&self) -> LlmTurnMetadata {
        let metadata = match self.execution_mode {
            AgentExecutionMode::Execute | AgentExecutionMode::Review => LlmTurnMetadata::execute(),
            AgentExecutionMode::Plan => LlmTurnMetadata::plan(),
        };
        if let Some(token) = self.cancellation_token.as_ref() {
            metadata.with_cancellation(token.clone())
        } else {
            metadata
        }
    }

    pub(super) fn model_event_label(&self) -> String {
        self.llm_backend
            .model_label()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }
}
