use crate::tui::command;
use crate::tui::state::Overlay;

use crate::runtime_client::{GoalContinuation, RuntimeClient};

#[cfg(test)]
pub(crate) async fn finish_running_task_if_ready(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
) -> anyhow::Result<()> {
    finish_running_task_if_ready_with_completion(app, agent_slot, None).await
}

pub(crate) async fn finish_running_task_if_ready_with_completion(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    completion: Option<Result<TaskCompletion, tokio::task::JoinError>>,
) -> anyhow::Result<()> {
    if app.bottom_pane.running_task.is_none() {
        return Ok(());
    }

    let (pending_events, is_finished) = {
        let task = app
            .bottom_pane
            .running_task
            .as_mut()
            .expect("task should exist");
        let mut pending_events = Vec::new();
        while let Ok(event) = task.receiver.try_recv() {
            pending_events.push(event);
        }
        let is_finished = completion.is_some() || task.handle.is_finished();
        (pending_events, is_finished)
    };

    for event in pending_events {
        apply_tui_event(app, event);
    }

    if !is_finished {
        emit_query_heartbeat(app);
        return Ok(());
    }

    let mut task = app
        .bottom_pane
        .running_task
        .take()
        .expect("task should exist");
    let completion = match completion {
        Some(completion) => completion?,
        None => task.handle.await?,
    };
    while let Ok(event) = task.receiver.try_recv() {
        apply_tui_event(app, event);
    }
    match completion {
        TaskCompletion::Query { agent, result } => {
            let mut agent = agent;
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
                    app.clear_active_live_sections();
                    if finished_plan_turn {
                        match RuntimeClient::plan_continuation(&agent, query_started_in_plan_mode) {
                            crate::runtime_client::PlanContinuation::AwaitApproval { tool_id } => {
                                app.show_pending_plan_approval(tool_id.as_deref());
                            }
                            crate::runtime_client::PlanContinuation::AutomaticImplementation => {
                                app.release_pending_follow_ups();
                                app.finalize_agent_stream(None);
                                start_automatic_plan_implementation_task(app, agent);
                                return Ok(());
                            }
                            crate::runtime_client::PlanContinuation::None => {
                                app.clear_pending_plan_approval();
                            }
                        }
                    }
                    if let Some(goal) = app.goal_handle.read().unwrap().clone() {
                        app.goal = Some(goal);
                    }
                    let prior_total_input_tokens = app.snapshot.total_input_tokens;
                    match RuntimeClient::continue_goal(
                            &app.goal_handle,
                            &mut agent,
                            prior_total_input_tokens,
                            finished_plan_turn,
                            app.has_pending_plan_approval(),
                        )
                        .await
                    {
                        GoalContinuation::BudgetLimited { goal, prompt } => {
                            app.goal = Some(goal.clone());
                            app.push_notice(format!(
                                "Goal budget exhausted: {} / {} tokens.",
                                goal.tokens_used,
                                goal.token_budget.unwrap_or(0)
                            ));
                            app.sync_snapshot(&agent);
                            app.finalize_active_turn();
                            *agent_slot = Some(agent);
                            start_query_task(app, prompt, agent_slot.take().expect("agent"));
                            return Ok(());
                        }
                        GoalContinuation::Complete { goal } => {
                            app.goal = Some(goal);
                            *agent_slot = Some(agent);
                            app.sync_snapshot(agent_slot.as_ref().expect("agent"));
                            app.release_pending_follow_ups();
                            app.finalize_agent_stream(None);
                            app.finalize_active_turn();
                            app.bottom_pane.notice =
                                Some("Goal evaluator marked the goal complete.".into());
                            app.set_runtime_phase(RuntimePhase::Idle, Some("goal complete".into()));
                            return Ok(());
                        }
                        GoalContinuation::Continue { goal, prompt, reason } => {
                            app.goal = Some(goal);
                            app.push_system(reason, crate::tui::state::SystemMessageKind::Other);
                            app.sync_snapshot(&agent);
                            app.finalize_active_turn();
                            start_query_task(app, prompt, agent);
                            return Ok(());
                        }
                        GoalContinuation::NotActive => {}
                    }
                    *agent_slot = Some(agent);
                    app.goal = app.goal_handle.read().unwrap().clone();
                    if let Some(a) = agent_slot.as_ref() {
                        app.sync_snapshot(a);
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
                        if finished_plan_turn {
                            app.push_notice("Planning finished. Staying in plan mode.");
                        }
                        app.finalize_active_turn();
                        if let Some(agent) = agent_slot.as_ref() {
                            crate::auto_memory::maybe_auto_memory(app, agent);
                        }
                        app.bottom_pane.notice = Some("Prompt finished.".into());
                        app.set_runtime_phase(RuntimePhase::Idle, Some("prompt finished".into()));
                        try_start_queued_follow_up(app, agent_slot);
                    }
                }
                Err(err) => {
                    let error_message = format_error_chain(&err);
                    let cancelled = error_message.contains("cancelled by user");
                    app.set_agent_execution_mode(agent.execution_mode);
                    let _finished_plan_turn = matches!(
                        app.agent_execution_mode,
                        crate::agent::AgentExecutionMode::Plan
                    );
                    app.clear_active_live_sections();

                    app.clear_pending_plan_approval();
                    *agent_slot = Some(agent);
                    if let Some(agent) = agent_slot.as_ref() {
                        app.sync_snapshot(agent);
                    }
                    app.release_pending_follow_ups();
                    app.finalize_agent_stream(None);
                    if cancelled {
                        app.finalize_active_turn();
                        app.bottom_pane.notice = Some("Query cancelled.".into());
                        app.set_runtime_phase(RuntimePhase::Idle, Some("query cancelled".into()));
                        try_start_queued_follow_up(app, agent_slot);
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
                    try_start_queued_follow_up(app, agent_slot);
                }
            }
        }
        TaskCompletion::Compact { agent, result } => {
            *agent_slot = Some(agent);
            if let Some(agent) = agent_slot.as_ref() {
                app.sync_snapshot(agent);
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
                        app.push_entry("Agent", message.clone());
                        app.push_notice(message);
                    } else {
                        app.push_entry("Agent", "Conversation compacted.");
                        app.push_notice("Conversation compacted.");
                    }
                    app.finalize_active_turn();
                    app.set_runtime_phase(RuntimePhase::Idle, Some("history compacted".into()));
                    try_start_queued_follow_up(app, agent_slot);
                }
                Ok(false) => {
                    app.clear_active_live_sections();
                    app.release_pending_follow_ups();
                    let message = "Conversation history did not need compaction.";
                    app.push_entry("Agent", message);
                    app.push_notice(message);
                    app.finalize_active_turn();
                    app.set_runtime_phase(RuntimePhase::Idle, Some("compact skipped".into()));
                    try_start_queued_follow_up(app, agent_slot);
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
                if let Some(goal) = app.goal.as_ref() {
                    *rebuilt.goal_handle.write().unwrap() = Some(goal.clone());
                }
                app.goal_handle = rebuilt.goal_handle;
                app.goal = app.goal_handle.read().unwrap().clone();
                app.mcp_tool_cache = Some(rebuilt.mcp_tool_cache);
                app.mcp_manager = Some(rebuilt.mcp_manager);
                app.lsp_manager = Some(rebuilt.lsp_manager);
                app.prompt_source_registry = Some(rebuilt.prompt_source_registry);
                app.skill_source_registry = Some(rebuilt.skill_source_registry);
                app.memory_handler = Some(rebuilt.memory_handler);
                app.hook_registry = Some(rebuilt.hook_registry);
                app.hook_runtime = Some(rebuilt.hook_runtime.clone());
                app.local_model_server = rebuilt.local_model_server;
                RuntimeClient::persist_config(&app.config_manager, &app.config)?;
                let is_bootstrap = app.setup_status.is_none();
                app.setup_status = Some(format!(
                    "Applied {} / {}",
                    app.config.provider,
                    app.current_model_label()
                ));
                app.bottom_pane.notice = app.setup_status.clone();
                *agent_slot = Some(agent);
                if let Some(agent) = agent_slot.as_ref() {
                    app.sync_snapshot(agent);
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
                app.finalize_active_turn();
                try_start_queued_follow_up(app, agent_slot);
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
                app.push_entry("Runtime", saved_message);
                start_rebuild_task(app);
            }
            Err(err) => {
                app.set_runtime_phase(RuntimePhase::Failed, Some("oauth failed".into()));
                let message = format!("OAuth failed:\n{}", format_error_chain(&err));
                app.push_system(message.clone(), SystemMessageKind::OAuth);
                app.push_notice(message);
            }
        },
        TaskCompletion::DeepSeekModels { result } => match result {
            Ok(models) => {
                let count = models.len();
                app.set_deepseek_model_options(models);
                app.bottom_pane.notice = Some(format!("Loaded {count} DeepSeek models."));
                app.set_runtime_phase(RuntimePhase::Idle, Some("models loaded".into()));
                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
            }
            Err(err) => {
                app.set_deepseek_model_options(fallback_models(ModelCatalogProvider::DeepSeek));
                let message = format!(
                    "Failed to load DeepSeek models. Showing fallback list.\n{}",
                    format_error_chain(&err)
                );
                app.push_system(message.clone(), SystemMessageKind::Other);
                app.push_notice(message);
                app.set_runtime_phase(RuntimePhase::Idle, Some("model list fallback".into()));
                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
            }
        },
        TaskCompletion::KimiModels { result } => match result {
            Ok(models) => {
                let count = models.len();
                app.set_kimi_model_options(models);
                app.bottom_pane.notice = Some(format!("Loaded {count} Kimi models."));
                app.set_runtime_phase(RuntimePhase::Idle, Some("models loaded".into()));
                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
            }
            Err(err) => {
                app.set_kimi_model_options(fallback_models(ModelCatalogProvider::Kimi));
                let message = format!(
                    "Failed to load Kimi models. Showing fallback list.\n{}",
                    format_error_chain(&err)
                );
                app.push_system(message.clone(), SystemMessageKind::Other);
                app.push_notice(message);
                app.set_runtime_phase(RuntimePhase::Idle, Some("model list fallback".into()));
                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
            }
        },
    }

    Ok(())
}

pub(super) fn emit_query_heartbeat(app: &mut TuiApp) -> bool {
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
