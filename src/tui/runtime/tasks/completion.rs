use super::*;
use crate::runtime_client::{GoalContinuation, RuntimeClient};
use crate::tui::command;
use crate::tui::message_role::MessageRole;
use crate::tui::runtime::permissions;
use crate::tui::state::Overlay;

#[cfg(test)]
pub(crate) async fn finish_running_task_if_ready(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
) -> anyhow::Result<()> {
    finish_running_task_if_ready_with_completion_mode(app, agent_slot, None, true, None).await
}

/// Complete a task after the structured runtime stream has already delivered
/// its events. The compatibility receiver is drained but not replayed, which
/// prevents each event from being applied twice during the port migration.
pub(crate) async fn finish_running_task_if_ready_from_runtime_port(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    completion: Option<Result<TaskCompletion, tokio::task::JoinError>>,
    runtime: Option<&mut RuntimeTaskServices>,
) -> anyhow::Result<()> {
    finish_running_task_if_ready_with_completion_mode(app, agent_slot, completion, false, runtime)
        .await
}

pub(super) async fn finish_running_task_if_ready_with_completion_mode(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    completion: Option<Result<TaskCompletion, tokio::task::JoinError>>,
    apply_compatibility_events: bool,
    mut runtime: Option<&mut RuntimeTaskServices>,
) -> anyhow::Result<()> {
    let (pending_events, is_finished) = {
        let Some(task) = app.bottom_pane.running_task.as_mut() else {
            return Ok(());
        };
        let mut pending_events = Vec::new();
        while let Ok(event) = task.receiver.try_recv() {
            pending_events.push(event);
        }
        let is_finished = completion.is_some() || task.handle.is_finished();
        (pending_events, is_finished)
    };

    if apply_compatibility_events {
        for event in pending_events {
            apply_tui_event(app, event);
        }
    }

    if !is_finished {
        emit_query_heartbeat(app);
        return Ok(());
    }

    let mut task = app
        .bottom_pane
        .running_task
        .take()
        .ok_or_else(|| anyhow::anyhow!("running task disappeared during event projection"))?;
    let completion = match completion {
        Some(completion) => completion,
        None => task.handle.await,
    };
    if apply_compatibility_events {
        while let Ok(event) = task.receiver.try_recv() {
            apply_tui_event(app, event);
        }
    } else {
        while task.receiver.try_recv().is_ok() {}
    }
    let completion = if matches!(task.kind, TaskKind::ReviewPreparation)
        && task
            .cancellation_token
            .as_ref()
            .is_some_and(|token| token.load(std::sync::atomic::Ordering::SeqCst))
    {
        if let Err(error) = &completion
            && !error.is_cancelled()
        {
            log::warn!("Review preparation failed while stopping: {error}");
        }
        if let Ok(TaskCompletion::ReviewPrepared { result: Err(error) }) = &completion {
            log::warn!("Review preparation failed while stopping: {error:#}");
        }
        Ok(TaskCompletion::ReviewPrepared {
            result: Ok(super::super::review::ReviewPreparation::Cancelled),
        })
    } else {
        completion
    };
    let completion = match completion {
        Ok(completion) => completion,
        Err(error) => {
            log::warn!("Runtime task failed to join: {error}");
            if agent_slot.is_none() {
                app.snapshot.pending_interactions.clear();
                app.clear_pending_planning_suggestion();
                app.persist_runtime_state();
            }
            app.release_pending_follow_ups();
            app.finalize_active_turn();
            app.set_runtime_phase(RuntimePhase::Failed, Some("runtime task failed".into()));
            let recovery = if agent_slot.is_none() {
                "Submit another prompt to rebuild the backend, or change the model."
            } else {
                "The current backend is still available; retry the operation."
            };
            let message = format!("Runtime task failed: {error}. {recovery}");
            app.push_notice(message);
            if let Some(mode) = app.pending_permission_mode.take() {
                app.push_notice(format!(
                    "Permissions not applied: {}. The task failed to return its runtime agent.",
                    mode.label()
                ));
            }
            return Ok(());
        }
    };
    match completion {
        TaskCompletion::ReviewPrepared { result } => {
            super::super::review::finish(app, agent_slot, result);
        }
        TaskCompletion::Query {
            mut agent,
            result,
            goal_turn,
        } => {
            let query_started_in_plan_mode = matches!(
                app.agent_execution_mode,
                crate::agent::AgentExecutionMode::Plan
            );
            if let Err(err) = RuntimeClient::persist_bash_prefixes(&app.config_manager, &agent) {
                app.push_notice(format!(
                    "Failed to persist bash approval rules: {}",
                    format_error_chain(&err)
                ));
            }
            match result {
                Ok(_) => {
                    app.set_agent_execution_mode(agent.execution_mode);
                    let finished_plan_turn = matches!(
                        app.agent_execution_mode,
                        crate::agent::AgentExecutionMode::Plan
                    );
                    let plan_continuation =
                        RuntimeClient::plan_continuation(&agent, query_started_in_plan_mode);
                    let permission_changed =
                        permissions::apply_pending_permission_mode(app, &mut agent);
                    app.clear_active_live_sections();
                    let continuation = match RuntimeClient::continue_goal(
                        &app.goal_handle,
                        &agent,
                        goal_turn.as_ref(),
                        finished_plan_turn,
                        app.active_pending_interaction().is_some(),
                    ) {
                        Ok(continuation) => continuation,
                        Err(error) => {
                            app.goal = app.goal_handle.snapshot();
                            app.apply_runtime_snapshot(
                                &agent,
                                RuntimeClient::extension_snapshot_for_agent(&agent, 0),
                            );
                            log::warn!("Goal accounting failed; stopping continuation: {error:#}");
                            app.push_notice(format!(
                                "Goal accounting failed; continuation stopped: {error:#}"
                            ));
                            app.finalize_active_turn();
                            app.set_runtime_phase(
                                RuntimePhase::Failed,
                                Some("goal persistence failed".into()),
                            );
                            *agent_slot = Some(agent);
                            return Ok(());
                        }
                    };
                    app.goal = app.goal_handle.snapshot();
                    if finished_plan_turn {
                        match plan_continuation {
                            crate::runtime_client::PlanContinuation::AwaitApproval { tool_id } => {
                                app.show_pending_plan_approval(tool_id.as_deref());
                            }
                            crate::runtime_client::PlanContinuation::AutomaticImplementation
                                if permission_changed =>
                            {
                                app.show_pending_plan_approval(None);
                            }
                            crate::runtime_client::PlanContinuation::AutomaticImplementation => {
                                app.release_pending_follow_ups();
                                app.finalize_agent_stream(None);
                                start_automatic_plan_implementation_task(
                                    app,
                                    agent,
                                    runtime.as_deref().cloned(),
                                );
                                return Ok(());
                            }
                            crate::runtime_client::PlanContinuation::None => {
                                app.clear_pending_plan_approval();
                            }
                        }
                    }
                    match continuation {
                        GoalContinuation::BudgetLimited { goal, prompt } => {
                            app.goal = Some(goal.clone());
                            app.push_notice(format!(
                                "Goal budget exhausted: {} / {} tokens.",
                                goal.tokens_used,
                                goal.token_budget.unwrap_or(0)
                            ));
                            app.apply_runtime_snapshot(
                                &agent,
                                crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(
                                    &agent, 0,
                                ),
                            );
                            app.finalize_active_turn();
                            if let Some(services) = runtime.as_deref().cloned() {
                                start_goal_continuation_task_with_services(
                                    app, prompt, agent, services,
                                );
                            } else {
                                start_goal_continuation_task(app, prompt, agent);
                            }
                            return Ok(());
                        }
                        GoalContinuation::Continue { .. }
                            if !app.bottom_pane.pending_follow_up_messages.is_empty()
                                || !app.bottom_pane.queued_follow_up_messages.is_empty() => {}
                        GoalContinuation::Continue { goal, prompt } => {
                            app.goal = Some(goal);
                            app.apply_runtime_snapshot(
                                &agent,
                                crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(
                                    &agent, 0,
                                ),
                            );
                            app.finalize_active_turn();
                            if let Some(services) = runtime.as_deref().cloned() {
                                start_goal_continuation_task_with_services(
                                    app, prompt, agent, services,
                                );
                            } else {
                                start_goal_continuation_task(app, prompt, agent);
                            }
                            return Ok(());
                        }
                        GoalContinuation::NotActive => {}
                    }
                    *agent_slot = Some(agent);
                    app.goal = app.goal_handle.snapshot();
                    if let Some(a) = agent_slot.as_ref() {
                        app.apply_runtime_snapshot(
                            a,
                            crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(
                                a, 0,
                            ),
                        );
                    }
                    app.release_pending_follow_ups();
                    app.finalize_agent_stream(None);
                    if finished_plan_turn && app.has_pending_plan_approval() {
                        app.bottom_pane.notice = Some("Plan ready for approval.".into());
                        app.set_runtime_phase(
                            RuntimePhase::Idle,
                            Some("awaiting plan approval".into()),
                        );
                    } else {
                        if finished_plan_turn
                            && app.agent_execution_mode == crate::agent::AgentExecutionMode::Plan
                        {
                            app.push_notice("Planning finished. Staying in plan mode.");
                        }
                        app.finalize_active_turn();
                        app.bottom_pane.notice = Some("Prompt finished.".into());
                        app.set_runtime_phase(RuntimePhase::Idle, Some("prompt finished".into()));
                        try_start_queued_follow_up(app, agent_slot, runtime.as_deref().cloned());
                    }
                }
                Err(err) => {
                    let error_message = format_error_chain(&err);
                    let stopped = task
                        .query_control
                        .as_ref()
                        .and_then(QueryTaskControl::stop_kind);
                    if stopped.is_some() {
                        agent.discard_pending_interactions();
                    }
                    app.set_agent_execution_mode(agent.execution_mode);
                    permissions::apply_pending_permission_mode(app, &mut agent);
                    app.clear_active_live_sections();

                    app.clear_pending_plan_approval();
                    *agent_slot = Some(agent);
                    if let Some(agent) = agent_slot.as_ref() {
                        app.apply_runtime_snapshot(
                            agent,
                            crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(
                                agent, 0,
                            ),
                        );
                    }
                    app.release_pending_follow_ups();
                    app.finalize_agent_stream(None);
                    if let Some(kind) = stopped {
                        app.finalize_active_turn();
                        let (notice, detail) = match kind {
                            QueryStopKind::Interrupt => ("Query interrupted.", "query interrupted"),
                            QueryStopKind::Cancel => ("Query cancelled.", "query cancelled"),
                        };
                        app.bottom_pane.notice = Some(notice.into());
                        app.set_runtime_phase(RuntimePhase::Idle, Some(detail.into()));
                        try_start_queued_follow_up(app, agent_slot, runtime.as_deref().cloned());
                        return Ok(());
                    }
                    app.set_runtime_phase(RuntimePhase::Failed, Some("query failed".into()));
                    let mut message = format!("Query failed:\n{error_message}");
                    if app.config.provider == "ollama" {
                        let base_url = app
                            .config
                            .base_url
                            .as_deref()
                            .unwrap_or("http://localhost:11434");
                        message.push_str(&format!(
                            "\nbase_url={}",
                            sanitize_url_for_display(base_url)
                        ));
                    }
                    app.push_system(message.clone(), SystemMessageKind::Other);
                    app.push_notice(message);
                    try_start_queued_follow_up(app, agent_slot, runtime.as_deref().cloned());
                }
            }
        }
        TaskCompletion::Compact { mut agent, result } => {
            permissions::apply_pending_permission_mode(app, &mut agent);
            *agent_slot = Some(agent);
            if let Some(agent) = agent_slot.as_ref() {
                app.apply_runtime_snapshot(
                    agent,
                    crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(agent, 0),
                );
            }
            match result {
                Ok(true) => {
                    app.clear_active_live_sections();
                    app.release_pending_follow_ups();
                    if let Some((before, after)) = app
                        .snapshot
                        .last_compaction_before_tokens
                        .zip(app.snapshot.last_compaction_after_tokens)
                    {
                        let message = format!(
                            "Conversation compacted.\nEstimated history tokens: {before} -> {after}"
                        );
                        app.push_entry(MessageRole::Agent, message.clone());
                        app.push_notice(message);
                    } else {
                        app.push_entry(MessageRole::Agent, "Conversation compacted.");
                        app.push_notice("Conversation compacted.");
                    }
                    app.finalize_active_turn();
                    app.set_runtime_phase(RuntimePhase::Idle, Some("history compacted".into()));
                    try_start_queued_follow_up(app, agent_slot, runtime.as_deref().cloned());
                }
                Ok(false) => {
                    app.clear_active_live_sections();
                    app.release_pending_follow_ups();
                    let message = "Conversation history did not need compaction.";
                    app.push_entry(MessageRole::Agent, message);
                    app.push_notice(message);
                    app.finalize_active_turn();
                    app.set_runtime_phase(RuntimePhase::Idle, Some("compact skipped".into()));
                    try_start_queued_follow_up(app, agent_slot, runtime.as_deref().cloned());
                }
                Err(err) => {
                    app.clear_active_live_sections();
                    app.release_pending_follow_ups();
                    app.set_runtime_phase(RuntimePhase::Failed, Some("compact failed".into()));
                    let message = format!("Compaction failed:\n{}", format_error_chain(&err));
                    app.push_system(message.clone(), SystemMessageKind::Other);
                    app.push_notice(message);
                }
            }
        }
        TaskCompletion::Rebuild { result } => match result {
            Ok(rebuilt) => {
                let mut agent = rebuilt.agent;
                if let Some(previous) = agent_slot.take() {
                    agent = merge_rebuilt_agent(agent, previous);
                }
                agent.set_execution_mode(app.agent_execution_mode);
                agent.set_bash_approval_mode(app.bash_approval_mode);
                agent.set_full_access_mode(app.permission_mode == PermissionMode::FullAccess);
                rebuilt.sandbox_network_access.store(
                    app.sandbox_network_access
                        .load(std::sync::atomic::Ordering::Relaxed),
                    std::sync::atomic::Ordering::Relaxed,
                );
                app.sandbox_network_access = rebuilt.sandbox_network_access;
                permissions::apply_pending_permission_mode(app, &mut agent);
                rebuilt.goal_handle.inherit_from(&app.goal_handle);
                app.goal_handle = rebuilt.goal_handle;
                app.goal = app.goal_handle.snapshot();
                app.mcp_tool_cache = Some(rebuilt.mcp_tool_cache);
                app.mcp_manager = Some(rebuilt.mcp_manager);
                app.lsp_manager = Some(rebuilt.lsp_manager);
                if let Some(runtime) = runtime.as_deref_mut() {
                    *runtime = RuntimeTaskServices {
                        prompt_source_registry: rebuilt.prompt_source_registry.clone(),
                        skill_source_registry: rebuilt.skill_source_registry.clone(),
                        hook_registry: rebuilt.hook_registry.clone(),
                    };
                }
                #[cfg(test)]
                {
                    app.prompt_source_registry = Some(rebuilt.prompt_source_registry);
                    app.skill_source_registry = Some(rebuilt.skill_source_registry);
                }
                app.memory_handler = Some(rebuilt.memory_handler);
                #[cfg(test)]
                {
                    app.hook_registry = Some(rebuilt.hook_registry);
                }
                app.hook_runtime = Some(rebuilt.hook_runtime.clone());
                let is_bootstrap = app.setup_status.is_none();
                app.setup_status = Some(format!(
                    "Applied {} / {}",
                    app.config.provider,
                    app.current_model_label()
                ));
                app.bottom_pane.notice = app.setup_status.clone();
                *agent_slot = Some(agent);
                if let Some(agent) = agent_slot.as_ref() {
                    app.apply_runtime_snapshot(
                        agent,
                        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(
                            agent, 0,
                        ),
                    );
                }
                app.dismiss_overlay();
                app.set_runtime_phase(RuntimePhase::BackendReady, Some("backend ready".into()));
                app.push_system(
                    app.setup_status.clone().unwrap_or_default(),
                    if is_bootstrap {
                        SystemMessageKind::BackendBootstrap
                    } else {
                        SystemMessageKind::BackendRebuild
                    },
                );
                let warning_count = rebuilt.warnings.len();
                let default_kind = if is_bootstrap {
                    SystemMessageKind::BackendBootstrap
                } else {
                    SystemMessageKind::BackendRebuild
                };
                for warning in rebuilt.warnings {
                    let kind = classify_system_warning(&warning, default_kind);
                    app.push_system(warning, kind);
                }
                if warning_count > 0 {
                    let notice = if warning_count == 1 {
                        "Startup warning added to transcript.".to_string()
                    } else {
                        format!("{warning_count} startup warnings added to transcript.")
                    };
                    app.bottom_pane.notice = Some(notice);
                }
                if let Err(error) = RuntimeClient::persist_config(&app.config_manager, &app.config)
                {
                    log::warn!("Backend applied but configuration was not saved: {error:#}");
                    let message =
                        format!("Backend applied, but configuration was not saved: {error:#}");
                    app.push_notice(message);
                }
                app.finalize_active_turn();
                try_start_queued_follow_up(app, agent_slot, runtime.as_deref().cloned());
            }
            Err(err) => {
                app.set_runtime_phase(RuntimePhase::Failed, Some("backend rebuild failed".into()));
                let message = format!("Failed to apply config:\n{}", format_error_chain(&err));
                app.setup_status = Some(message.clone());
                app.push_notice(message);
            }
        },
        TaskCompletion::OAuth { mode, result } => match result {
            Ok(credential) => {
                app.config.set_provider("codex");
                app.config
                    .set_api_key(credential.expose_secret().to_string());
                app.codex_auth_mode = Some(crate::oauth::SavedCodexAuthMode::Chatgpt);
                let base_url = match mode {
                    OAuthLoginMode::Browser | OAuthLoginMode::DeviceCode => {
                        crate::config::DEFAULT_CODEX_CHATGPT_BASE_URL
                    }
                };
                app.config.apply_codex_defaults_for_base_url(base_url);
                app.config_manager.save(&app.config)?;
                let saved_message = match mode {
                    OAuthLoginMode::Browser => {
                        "Saved Codex browser login credential to local config."
                    }
                    OAuthLoginMode::DeviceCode => {
                        "Saved Codex device-code login credential to local config."
                    }
                };
                app.setup_status = Some(saved_message.into());
                app.bottom_pane.notice = app.setup_status.clone();
                app.set_runtime_phase(RuntimePhase::OAuthSaved, Some("oauth token saved".into()));
                app.dismiss_overlay();
                app.push_entry(MessageRole::Runtime, saved_message);
                start_rebuild_task(app, agent_slot.as_ref().and_then(Agent::agent_tree_control));
            }
            Err(err) => {
                app.set_runtime_phase(RuntimePhase::Failed, Some("oauth failed".into()));
                let message = format!("OAuth failed:\n{}", format_error_chain(&err));
                app.push_system(message.clone(), SystemMessageKind::OAuth);
                app.push_notice(message);
            }
        },
        TaskCompletion::ModelCatalog { provider, result } => match result {
            Ok(models) => {
                let count = models.len();
                match provider {
                    ModelCatalogProvider::DeepSeek => app.set_deepseek_model_catalog(models),
                    ModelCatalogProvider::Kimi => app.set_kimi_model_catalog(models),
                }
                let label = match provider {
                    ModelCatalogProvider::DeepSeek => "DeepSeek",
                    ModelCatalogProvider::Kimi => "Moonshot AI",
                };
                app.bottom_pane.notice = Some(format!("Loaded {count} {label} models."));
                app.set_runtime_phase(RuntimePhase::Idle, Some("models loaded".into()));
                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
            }
            Err(err) => {
                let fallback = fallback_catalog(provider);
                match provider {
                    ModelCatalogProvider::DeepSeek => {
                        app.set_deepseek_model_catalog_with_source(fallback, true)
                    }
                    ModelCatalogProvider::Kimi => {
                        app.set_kimi_model_catalog_with_source(fallback, true)
                    }
                }
                let label = match provider {
                    ModelCatalogProvider::DeepSeek => "DeepSeek",
                    ModelCatalogProvider::Kimi => "Moonshot AI",
                };
                let message = format!(
                    "Failed to load {label} models. Showing fallback list.\n{}",
                    format_error_chain(&err)
                );
                app.push_system(message.clone(), SystemMessageKind::Other);
                app.push_notice(message);
                app.set_runtime_phase(RuntimePhase::Idle, Some("model list fallback".into()));
                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
            }
        },
    }

    if !app.is_busy()
        && let Some(mode) = app.pending_permission_mode.take()
    {
        permissions::request_permission_mode(app, agent_slot, mode);
    }
    Ok(())
}

pub(crate) fn emit_query_heartbeat(app: &mut TuiApp) -> bool {
    let elapsed = {
        let Some(task) = app.bottom_pane.running_task.as_mut() else {
            return false;
        };
        if !matches!(task.kind, TaskKind::Query) {
            return false;
        }

        let elapsed = task.started_at.elapsed().as_secs();
        if elapsed < task.next_heartbeat_after_secs {
            return false;
        }
        task.next_heartbeat_after_secs = elapsed.saturating_add(1);
        elapsed
    };

    let is_local = command::is_local_provider(&app.config.provider);
    let current_detail = app
        .runtime_phase_detail
        .as_deref()
        .map(|detail| detail.split(" · ").next().unwrap_or(detail))
        .filter(|detail| !detail.trim().is_empty());
    let (phase, detail, notice) = match app.runtime_phase {
        RuntimePhase::RunningTool => {
            let detail = format!(
                "{} · {}s elapsed",
                current_detail.unwrap_or("running tool"),
                elapsed
            );
            (
                RuntimePhase::RunningTool,
                detail.clone(),
                format!("Running tool · {}s elapsed", elapsed),
            )
        }
        RuntimePhase::ProcessingResponse => {
            let detail = format!(
                "{} · {}s elapsed",
                current_detail.unwrap_or("processing response"),
                elapsed
            );
            (
                RuntimePhase::ProcessingResponse,
                detail.clone(),
                format!("Processing response · {}s elapsed", elapsed),
            )
        }
        _ => {
            let detail = if is_local {
                format!("local model is still generating · {}s elapsed", elapsed)
            } else {
                format!("waiting for model response · {}s elapsed", elapsed)
            };
            let notice = if is_local {
                format!("Working locally · {}s elapsed", elapsed)
            } else {
                format!("Waiting on {} · {}s elapsed", app.config.provider, elapsed)
            };
            (RuntimePhase::SendingPrompt, detail, notice)
        }
    };

    app.set_runtime_phase(phase, Some(detail));
    app.bottom_pane.notice = Some(notice);
    true
}
