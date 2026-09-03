use super::*;

#[allow(clippy::too_many_arguments)]
// Sub-agent execution is called from multiple tool entrypoints with explicit
// runtime handles; grouping them would obscure the execution boundary.
pub(crate) async fn run_sub_agent(
    kind: SubAgentKind,
    agent_id: &str,
    definition: Option<&AgentDefinition>,
    name: Option<&str>,
    parent_session_id: Option<&str>,
    instruction: &str,
    session_id: Option<String>,
    cancellation_token: Option<Arc<AtomicBool>>,
    model_target: Option<SubagentProviderTarget>,
    backend: Arc<dyn LlmBackend>,
    backend_resolver: Arc<dyn SubagentBackendResolver>,
    memory_handle: Arc<MemoryHandle>,
    session_manager: Arc<SessionManager>,
    workspace: Arc<WorkspaceMemory>,
    prompt_config: PromptRuntimeConfig,
    task_list_id: String,
    agent_definitions: AgentDefinitionCache,
    skill_manager: Option<Arc<RwLock<SkillManager>>>,
    agent_tree_control: Option<Arc<AgentTreeControl>>,
) -> Result<SubAgentResult, ToolError> {
    let permission_mode = agent_permission_mode(definition)?;
    let token_budget = agent_token_budget(definition)?;
    let resolved_backend = backend_resolver
        .resolve_backend(model_target.as_ref(), backend)
        .await?;
    if let Some(control) = agent_tree_control.as_ref() {
        control.record_model_resolution(
            agent_id,
            &resolved_backend.provider,
            &resolved_backend.model,
        )?;
    }
    let tool_manager = if let Some(def) = definition {
        build_filtered_tool_manager(kind, def, workspace.rara_dir.join("tasks"), &task_list_id)
    } else {
        Ok(build_subagent_tool_manager(
            kind,
            workspace.rara_dir.join("tasks"),
            &task_list_id,
        ))
    }?;
    let capability_policy = SubagentPluginCapabilityPolicy {
        plugin_skills: definition
            .map(|definition| definition.plugin_skills.clone())
            .unwrap_or_default(),
        ..Default::default()
    };
    let skill_tool_enabled = definition.is_none_or(|definition| {
        let included = definition.tools.is_empty()
            || definition
                .tools
                .iter()
                .map(|name| agent_tool_to_internal_name(name))
                .any(|name| name == "skill");
        let excluded = definition
            .disallowed_tools
            .iter()
            .map(|name| agent_tool_to_internal_name(name))
            .any(|name| name == "skill");
        included && !excluded
    });
    if kind.read_only() && !capability_policy.plugin_skills.is_empty() {
        return Err(ToolError::InvalidInput(
            "pluginSkills are not supported for read-only subagents".into(),
        ));
    }
    if !skill_tool_enabled && !capability_policy.plugin_skills.is_empty() {
        return Err(ToolError::InvalidInput(
            "pluginSkills requires the skill tool to be enabled".into(),
        ));
    }
    let mut tool_manager = tool_manager;
    register_scoped_plugin_skill_tool(
        &mut tool_manager,
        skill_manager,
        &capability_policy.plugin_skills,
    )?;
    let mut sub = Agent::new_with_agent_definitions(
        tool_manager,
        resolved_backend.backend,
        memory_handle,
        session_manager.clone(),
        workspace.clone(),
        agent_definitions,
    );
    if let Some(session_id) = session_id {
        sub.session_id = session_id;
    }
    sub.set_agent_tree_control(agent_tree_control.clone());
    sub.set_cancellation_token(cancellation_token);
    let plan_required =
        definition.is_some_and(|d| d.plan_mode_required) || permission_mode.requires_plan_mode();
    sub.set_execution_mode(if plan_required {
        AgentExecutionMode::Plan
    } else {
        kind.execution_mode()
    });
    sub.set_bash_approval_mode(permission_mode.bash_approval_mode(plan_required));
    sub.set_full_access_mode(permission_mode.full_access_mode(plan_required));
    sub.set_token_budget(token_budget);
    let role_prompt = subagent_role_prompt(kind, definition);
    let appended_prompt = match definition
        .map(|d| d.system_prompt.trim())
        .filter(|prompt| !prompt.is_empty())
    {
        Some(system_prompt) => format!("{role_prompt}\n\n{system_prompt}"),
        None => role_prompt,
    };
    let mut prompt_config = append_subagent_prompt(prompt_config, &appended_prompt);
    prompt_config.subagent_capability_policy = Some(capability_policy.prompt_instructions());
    sub.set_prompt_config(prompt_config);
    sub.task_list_id = task_list_id;

    let def_max_turns = definition
        .and_then(|d| {
            if d.max_turns > 0 {
                Some(d.max_turns)
            } else {
                None
            }
        })
        .unwrap_or_else(|| kind.default_max_turns());
    sub.set_max_turns(def_max_turns);

    let progress_agent_id = agent_id.to_string();
    let progress_control = agent_tree_control;
    let query_fut = sub.query_with_mode_and_events(
        instruction.to_string(),
        crate::agent::AgentOutputMode::Silent,
        move |event| {
            if let Some(control) = progress_control.as_ref()
                && let Err(error) = control.record_progress_event(&progress_agent_id, &event)
            {
                log::warn!("failed to record progress for sub-agent {progress_agent_id}: {error}");
            }
        },
    );

    tokio::time::timeout(Duration::from_secs(SUBAGENT_TIMEOUT_SECS), query_fut)
        .await
        .map_err(|_elapsed| {
            ToolError::ExecutionFailed(format!(
                "sub-agent {} ({}) timed out after {} seconds",
                agent_id,
                kind.label(),
                SUBAGENT_TIMEOUT_SECS
            ))
        })?
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    let status = if sub.token_budget_exhausted {
        "budget_limited"
    } else {
        kind.result_status()
    };
    let mut summary =
        latest_assistant_text(&sub).unwrap_or_else(|| "Sub-agent finished.".to_string());
    if sub.token_budget_exhausted {
        let budget = sub.token_budget.unwrap_or_default();
        let used = sub.total_model_tokens();
        summary = format!("Token budget exhausted: {used} / {budget} tokens. {summary}");
    }

    let persistence_error = parent_session_id.and_then(|parent_session_id| {
        persist_subagent_edge(
            &session_manager,
            &workspace,
            parent_session_id,
            agent_id,
            name,
            &sub,
            status,
            &summary,
            token_budget,
            &resolved_backend.provider,
            &resolved_backend.model,
        )
        .err()
        .map(|err| err.to_string())
    });

    Ok(SubAgentResult {
        agent_id: agent_id.to_string(),
        session_id: sub.session_id.clone(),
        total_input_tokens: sub.total_input_tokens,
        total_output_tokens: sub.total_output_tokens,
        total_cache_hit_tokens: sub.total_cache_hit_tokens,
        total_cache_miss_tokens: sub.total_cache_miss_tokens,
        status,
        summary,
        provider: resolved_backend.provider,
        model: resolved_backend.model,
        token_budget,
        token_budget_exhausted: sub.token_budget_exhausted,
        persistence_error,
        plan: (!sub.current_plan.is_empty()).then_some(sub.current_plan.clone()),
        plan_explanation: sub.plan_explanation.clone(),
        request_user_input: sub.pending_user_input.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
// Persistence mirrors the spawn-agent rollout edge fields.
pub(super) fn persist_subagent_edge(
    session_manager: &SessionManager,
    workspace: &WorkspaceMemory,
    parent_session_id: &str,
    agent_id: &str,
    name: Option<&str>,
    sub: &Agent,
    status: &str,
    summary: &str,
    token_budget: Option<u32>,
    provider: &str,
    model: &str,
) -> anyhow::Result<()> {
    write_subagent_sidechain(session_manager, parent_session_id, agent_id, sub)?;
    persist_subagent_runtime_state(
        session_manager,
        workspace,
        parent_session_id,
        sub,
        provider,
        model,
    )?;
    session_manager.save_spawn_agent_event(
        parent_session_id,
        &format!("spawn-{}", uuid::Uuid::new_v4()),
        agent_id,
        name,
        &sub.session_id,
        status,
        Some(summary),
        token_budget,
    )
}

pub(super) fn write_subagent_sidechain(
    session_manager: &SessionManager,
    parent_session_id: &str,
    agent_id: &str,
    sub: &Agent,
) -> anyhow::Result<()> {
    let path = session_transcript::subagent_transcript_path(
        &session_manager.storage_dir,
        parent_session_id,
        agent_id,
    );
    let scope = TranscriptScope::sidechain(parent_session_id, agent_id, sub.session_id.clone());
    session_transcript::write_message_snapshot(&path, &scope, &sub.history)
}

pub(super) fn persist_subagent_runtime_state(
    session_manager: &SessionManager,
    workspace: &WorkspaceMemory,
    parent_session_id: &str,
    sub: &Agent,
    provider: &str,
    model: &str,
) -> anyhow::Result<()> {
    let rara_dir = session_manager
        .storage_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("session storage dir has no parent"))?;
    let state_db = StateDb::new_for_root_dir(rara_dir.to_path_buf())?;
    let recorder = ThreadRecorder::new(&state_db);
    let (cwd, branch) = workspace.get_env_info();

    recorder.persist_runtime_state_with_lineage(
        &ThreadRuntimeState {
            session_id: &sub.session_id,
            cwd: &cwd,
            branch: &branch,
            provider,
            model,
            base_url: None,
            agent_mode: sub.execution_mode_label(),
            bash_approval: "unavailable",
            plan_explanation: sub.plan_explanation.as_deref(),
            prompt_runtime: PersistedPromptRuntimeState::default(),
            history_len: sub.history.len(),
            transcript_len: sub.history.len(),
            compact_state: PersistedCompactState::default(),
        },
        &ThreadRuntimeLineage {
            origin_kind: "subagent".to_string(),
            forked_from_thread_id: Some(parent_session_id.to_string()),
        },
    )?;

    recorder.replace_plan_steps(&sub.session_id, &persisted_plan_steps(&sub.current_plan))?;
    recorder.replace_interactions(
        &sub.session_id,
        &persisted_pending_interactions(sub.pending_user_input.as_ref()),
    )?;

    Ok(())
}

pub(super) fn build_read_only_tool_manager(
    task_store: Arc<TaskListStore>,
    default_task_list_id: &str,
) -> ToolManager {
    // Keep this registration set synchronized with strict_read_only_subagent_prompt!().
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ReadFileTool::default()));
    tool_manager.register(Box::new(ListFilesTool));
    tool_manager.register(Box::new(GlobTool));
    tool_manager.register(Box::new(GrepTool));
    tool_manager.register(Box::new(TaskListTool {
        store: task_store.clone(),
        default_task_list_id: default_task_list_id.to_string(),
    }));
    tool_manager.register(Box::new(TaskGetTool {
        store: task_store,
        default_task_list_id: default_task_list_id.to_string(),
    }));
    tool_manager
}

pub(crate) fn build_subagent_tool_manager(
    kind: SubAgentKind,
    task_root: PathBuf,
    default_task_list_id: &str,
) -> ToolManager {
    let task_store = Arc::new(TaskListStore::new(task_root));
    if kind.read_only() {
        build_read_only_tool_manager(task_store, default_task_list_id)
    } else {
        let mut tool_manager = ToolManager::new();
        tool_manager.register(Box::new(TaskCreateTool {
            store: task_store.clone(),
            default_task_list_id: default_task_list_id.to_string(),
        }));
        tool_manager.register(Box::new(TaskListTool {
            store: task_store.clone(),
            default_task_list_id: default_task_list_id.to_string(),
        }));
        tool_manager.register(Box::new(TaskUpdateTool {
            store: task_store.clone(),
            default_task_list_id: default_task_list_id.to_string(),
        }));
        tool_manager.register(Box::new(TaskGetTool {
            store: task_store,
            default_task_list_id: default_task_list_id.to_string(),
        }));
        tool_manager
    }
}

pub(super) fn build_custom_spawn_agent_tool_manager(
    task_root: PathBuf,
    default_task_list_id: &str,
) -> ToolManager {
    let task_store = Arc::new(TaskListStore::new(task_root));
    let mut tool_manager = build_read_only_tool_manager(task_store.clone(), default_task_list_id);
    tool_manager.register(Box::new(WebFetchTool));
    tool_manager.register(Box::new(WebSearchTool::from_env()));
    tool_manager.register(Box::new(TaskCreateTool {
        store: task_store.clone(),
        default_task_list_id: default_task_list_id.to_string(),
    }));
    tool_manager.register(Box::new(TaskUpdateTool {
        store: task_store,
        default_task_list_id: default_task_list_id.to_string(),
    }));
    tool_manager
}

pub(super) fn normalize_team_tasks(tasks: &[Value]) -> Result<Vec<TeamTask>, ToolError> {
    tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| {
            let name = normalize_team_task_name(idx, task)?;
            let instruction = task["instruction"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| ToolError::InvalidInput(format!("tasks[{idx}].instruction")))?;
            let kind = match task.get("kind") {
                Some(value) => {
                    let kind = value.as_str().ok_or_else(|| {
                        ToolError::InvalidInput(format!("tasks[{idx}].kind must be a string"))
                    })?;
                    parse_team_task_kind(idx, Some(kind))?
                }
                None => parse_team_task_kind(idx, None)?,
            };
            let provider = optional_string_field(task, "provider")
                .map_err(|field| ToolError::InvalidInput(format!("tasks[{idx}].{field}")))?;
            let model = optional_string_field(task, "model")
                .map_err(|field| ToolError::InvalidInput(format!("tasks[{idx}].{field}")))?;
            let definition = resolve_kind_definition(kind);
            let model_target = if provider.is_some() || model.is_some() {
                provider_target_from_parts(provider.as_deref(), model.as_deref())?
            } else {
                model_target_from_definition(Some(&definition))?
            };
            Ok(TeamTask {
                name,
                instruction,
                kind,
                definition,
                model_target,
            })
        })
        .collect()
}

pub(super) fn optional_string_field(
    value: &Value,
    field: &'static str,
) -> Result<Option<String>, &'static str> {
    let Some(raw) = value.get(field) else {
        return Ok(None);
    };
    match raw.as_str() {
        Some(text) => Ok(Some(text.to_string())),
        None => Err(field),
    }
}

pub(super) fn normalize_team_task_name(idx: usize, task: &Value) -> Result<String, ToolError> {
    match task.get("name") {
        Some(value) => {
            let name = value.as_str().ok_or_else(|| {
                ToolError::InvalidInput(format!("tasks[{idx}].name must be a string"))
            })?;
            if validate_agent_id_label(name).is_none() {
                return Err(ToolError::InvalidInput(format!(
                    "tasks[{idx}].name must normalize to a non-empty agent id label"
                )));
            }
            Ok(name.to_string())
        }
        None => Ok(format!("worker-{}", idx + 1)),
    }
}

pub(super) fn parse_team_task_kind(
    idx: usize,
    kind: Option<&str>,
) -> Result<SubAgentKind, ToolError> {
    match kind.unwrap_or("explore") {
        "general" => Ok(SubAgentKind::General),
        "explore" => Ok(SubAgentKind::Explore),
        "plan" => Ok(SubAgentKind::Plan),
        other => Err(ToolError::InvalidInput(format!(
            "tasks[{idx}].kind must be one of general, explore, or plan; got {other}"
        ))),
    }
}

pub(super) fn serialize_team_result(name: &str, result: SubAgentResult) -> Value {
    json!({
        "agent_id": result.agent_id,
        "session_id": result.session_id,
        "name": name,
        "status": result.status,
        "summary": result.summary,
        "provider": result.provider,
        "model": result.model,
        "cache_hit_tokens": result.total_cache_hit_tokens,
        "cache_miss_tokens": result.total_cache_miss_tokens,
        "token_budget": result.token_budget,
        "token_budget_exhausted": result.token_budget_exhausted,
        "persistence_error": result.persistence_error,
        "plan": result.plan.as_ref().map(|steps| serialize_plan_steps(steps)),
        "plan_explanation": result.plan_explanation,
        "request_user_input": result
            .request_user_input
            .as_ref()
            .map(serialize_pending_user_input),
    })
}

pub(super) fn next_subagent_id(kind: SubAgentKind, name: Option<&str>) -> String {
    let label = name
        .and_then(validate_agent_id_label)
        .unwrap_or_else(|| kind.label().to_string());
    format!("{label}-{}", uuid::Uuid::new_v4())
}

pub(super) fn validate_agent_id_label(value: &str) -> Option<String> {
    let label = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    (!label.is_empty()).then_some(label)
}

pub(super) fn model_target_from_definition(
    definition: Option<&AgentDefinition>,
) -> Result<Option<SubagentProviderTarget>, ToolError> {
    let provider = definition.and_then(|definition| definition.provider.as_deref());
    let model = definition.and_then(|definition| definition.model.as_deref());
    provider_target_from_parts(provider, model)
}

pub(super) fn model_target_from_input(
    input: &Value,
    definition: Option<&AgentDefinition>,
) -> Result<Option<SubagentProviderTarget>, ToolError> {
    let has_provider = input.get("provider").is_some();
    let has_model = input.get("model").is_some();
    let provider = optional_string_field(input, "provider")
        .map_err(|field| ToolError::InvalidInput(field.to_string()))?;
    let model = optional_string_field(input, "model")
        .map_err(|field| ToolError::InvalidInput(field.to_string()))?;

    if !has_provider && !has_model {
        return model_target_from_definition(definition);
    }
    if has_provider {
        return provider_target_from_parts(provider.as_deref(), model.as_deref());
    }

    let model = normalize_inherited_override(model.as_deref(), "model")?;
    let Some(model) = model else {
        return Ok(None);
    };
    if model.contains(':') {
        return provider_target_from_parts(None, Some(&model));
    }
    let definition_provider = definition.and_then(|definition| definition.provider.as_deref());
    provider_target_from_parts(definition_provider, Some(&model))
}

pub(super) fn provider_target_from_parts(
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Option<SubagentProviderTarget>, ToolError> {
    let provider = normalize_inherited_override(provider, "provider")?;
    let model = normalize_inherited_override(model, "model")?;
    let Some(model) = model else {
        return Ok(provider.map(|provider| SubagentProviderTarget {
            provider: Some(provider),
            model: None,
        }));
    };

    if let Some((provider_from_model, model_from_model)) = model.split_once(':') {
        if provider.is_some() {
            return Err(ToolError::InvalidInput(
                "model must not use provider:model when provider is also set".into(),
            ));
        }
        let provider_from_model =
            normalize_required_override(provider_from_model, "model provider")?;
        let model_from_model = normalize_required_override(model_from_model, "model")?;
        return Ok(Some(SubagentProviderTarget {
            provider: Some(provider_from_model),
            model: Some(model_from_model),
        }));
    }

    Ok(Some(SubagentProviderTarget {
        provider,
        model: Some(model),
    }))
}

pub(super) fn normalize_inherited_override(
    value: Option<&str>,
    field: &str,
) -> Result<Option<String>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("inherit") {
        return Ok(None);
    }
    Ok(Some(normalize_required_override(value, field)?))
}

pub(super) fn normalize_required_override(value: &str, field: &str) -> Result<String, ToolError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "{field} must not be empty"
        )));
    }
    Ok(value.to_string())
}

pub(crate) fn append_subagent_prompt(
    mut prompt_config: PromptRuntimeConfig,
    appended_instructions: &str,
) -> PromptRuntimeConfig {
    if appended_instructions.trim().is_empty() {
        return prompt_config;
    }
    prompt_config.append_system_prompt = Some(match prompt_config.append_system_prompt.take() {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{existing}\n\n{appended_instructions}")
        }
        _ => appended_instructions.to_string(),
    });
    prompt_config
}

pub(super) fn latest_assistant_text_from_history(history: &[Message]) -> Option<String> {
    history.iter().rev().find_map(|message| {
        if message.role != "assistant" {
            return None;
        }
        if let Some(text) = message.content.as_str() {
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        message.content.as_array().and_then(|blocks| {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    block
                        .get("type")
                        .and_then(Value::as_str)
                        .zip(block.get("text").and_then(Value::as_str))
                })
                .filter_map(|(kind, text)| (kind == "text").then_some(text))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
            (!text.is_empty()).then_some(text)
        })
    })
}

pub(super) fn latest_assistant_text(agent: &Agent) -> Option<String> {
    latest_assistant_text_from_history(&agent.history)
}

pub(super) fn serialize_plan_steps(steps: &[PlanStep]) -> Vec<Value> {
    steps
        .iter()
        .map(|step| {
            json!({
                "step": step.step,
                "status": plan_step_status_label(&step.status),
            })
        })
        .collect()
}

pub(super) fn persisted_plan_steps(steps: &[PlanStep]) -> Vec<PersistedPlanStep> {
    steps
        .iter()
        .enumerate()
        .map(|(step_index, step)| PersistedPlanStep {
            step_index,
            status: plan_step_status_label(&step.status).to_string(),
            step: step.step.clone(),
        })
        .collect()
}

pub(super) fn plan_step_status_label(status: &PlanStepStatus) -> &'static str {
    match status {
        PlanStepStatus::Pending => "pending",
        PlanStepStatus::InProgress => "in_progress",
        PlanStepStatus::Completed => "completed",
    }
}

pub(super) fn serialize_pending_user_input(request: &PendingUserInput) -> Value {
    json!({
        "question": request.question,
        "options": request.options,
        "note": request.note,
    })
}

pub(super) fn persisted_pending_interactions(
    request: Option<&PendingUserInput>,
) -> Vec<PersistedInteraction> {
    request
        .map(|request| {
            vec![PersistedInteraction {
                kind: "request_user_input".to_string(),
                status: "pending".to_string(),
                title: request.question.clone(),
                summary: request.note.clone().unwrap_or_default(),
                payload: Some(serialize_pending_user_input(request)),
            }]
        })
        .unwrap_or_default()
}

pub(super) fn resolve_kind_definition(kind: SubAgentKind) -> AgentDefinition {
    builtin_agent_definition(kind.label()).unwrap_or(AgentDefinition {
        token_budget: None,
        name: kind.label().to_string(),
        description: kind.label().to_string(),
        model: None,
        provider: None,
        tools: vec![],
        disallowed_tools: vec![],
        plugin_skills: vec![],
        max_turns: 0,
        plan_mode_required: matches!(kind, SubAgentKind::Plan),
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    })
}

pub(super) fn resolve_spawn_agent_definition(
    cache: &AgentDefinitionCache,
    normalized_name: &str,
) -> AgentDefinition {
    cache
        .resolve(normalized_name)
        .unwrap_or_else(|| fallback_spawn_agent_definition(normalized_name))
}

pub(super) fn subagent_role_prompt(
    kind: SubAgentKind,
    definition: Option<&AgentDefinition>,
) -> String {
    if matches!(kind, SubAgentKind::General) && definition.is_some_and(|d| !d.tools.is_empty()) {
        return concat!(
            "## Sub-Agent Role\n",
            "- You are a custom workspace sub-agent.\n",
            "- Treat the assigned instruction as the complete task contract.\n",
            "- Honor every constraint in the assigned instruction, including workspace, branch, network, and output limits.\n",
            "- Stay inside the current workspace unless the assigned instruction explicitly allows another path.\n",
            "- Inspect repository or web evidence only through the read-only tools exposed to you.\n",
            "- You may use shared task-list tools to inspect, claim, update, or complete project tasks when they are exposed.\n",
            "- You do not have shell, editing, patching, interactive browser automation, or agent-spawning tools in this role.\n",
            "- If the assigned instruction requires unavailable tools, report the limitation and answer from the available context.\n",
            "- Do not delegate to another agent or spawn sub-agents; complete the assigned work directly."
        )
        .to_string();
    }
    kind.append_prompt().to_string()
}

pub(super) fn fallback_spawn_agent_definition(name: &str) -> AgentDefinition {
    AgentDefinition {
        token_budget: None,
        name: name.to_string(),
        description: name.to_string(),
        model: None,
        provider: None,
        tools: vec![],
        disallowed_tools: vec![],
        plugin_skills: vec![],
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    }
}

pub(super) fn register_scoped_plugin_skill_tool(
    tool_manager: &mut ToolManager,
    parent_skill_manager: Option<Arc<RwLock<SkillManager>>>,
    allowed_skills: &[String],
) -> Result<(), ToolError> {
    if allowed_skills.is_empty() {
        return Ok(());
    }

    let parent_skill_manager = parent_skill_manager.ok_or_else(|| {
        ToolError::ExecutionFailed(
            "plugin skills require a runtime-owned skill manager".to_string(),
        )
    })?;
    let parent = parent_skill_manager
        .read()
        .map_err(|err| ToolError::ExecutionFailed(format!("skill lock failed: {err}")))?;
    let mut scoped = SkillManager::new();
    for name in allowed_skills {
        let skill = parent
            .get_skill(name)
            .ok_or_else(|| ToolError::InvalidInput(format!("unknown plugin skill: {name}")))?;
        if skill.scope != SkillScope::Plugin {
            return Err(ToolError::InvalidInput(format!(
                "skill is not a plugin skill: {name}"
            )));
        }
        scoped.skills.insert(name.clone(), skill.clone());
    }

    tool_manager.register(Box::new(SkillTool {
        skill_manager: Arc::new(RwLock::new(scoped)),
        plugin_roots: Vec::new(),
        reload_policy: SkillReloadPolicy::Disabled,
    }));
    Ok(())
}

pub(super) fn build_filtered_tool_manager(
    kind: SubAgentKind,
    definition: &AgentDefinition,
    task_root: PathBuf,
    default_task_list_id: &str,
) -> Result<ToolManager, ToolError> {
    let permission_mode =
        parse_agent_permission_mode(definition.permission_mode.as_deref().unwrap_or_default())?;
    let force_read_only = definition.plan_mode_required || permission_mode.requires_plan_mode();
    let mut tm = if force_read_only {
        let task_store = Arc::new(TaskListStore::new(task_root));
        build_read_only_tool_manager(task_store, default_task_list_id)
    } else if matches!(kind, SubAgentKind::General) && !definition.tools.is_empty() {
        build_custom_spawn_agent_tool_manager(task_root, default_task_list_id)
    } else {
        build_subagent_tool_manager(kind, task_root, default_task_list_id)
    };

    if !definition.tools.is_empty() {
        let allowed: std::collections::HashSet<&str> = definition
            .tools
            .iter()
            .map(|s| agent_tool_to_internal_name(s))
            .collect();
        tm.retain(|name| allowed.contains(name));
    }
    if !definition.disallowed_tools.is_empty() {
        let blocked: std::collections::HashSet<&str> = definition
            .disallowed_tools
            .iter()
            .map(|s| agent_tool_to_internal_name(s))
            .collect();
        tm.retain(|name| !blocked.contains(name));
    }

    Ok(tm)
}
