use anyhow::Result;
#[cfg(test)]
use {crate::thread_store::ThreadStore, rara_state::state_db::StateDb, std::sync::Arc};

use super::state::{TranscriptEntry, TranscriptTurn, TuiApp};
use crate::agent::{
    Agent, AgentExecutionMode, BashApprovalMode, CompactBoundaryMetadata, CompletedInteraction,
    PendingApproval, PendingUserInput, PlanStep, PlanStepStatus, latest_compact_boundary_metadata,
};
use crate::thread_store::CompactionRecord;
use crate::tools::bash::BashCommandInput;
#[cfg(test)]
use crate::tui::message_role::MessageRole;
use crate::tui::state::NoticeLevel;

#[cfg(test)]
mod recovery_tests;

#[cfg(test)]
mod approval_tests;

mod loading;
pub(crate) use loading::PendingRestore;
pub(super) use loading::{
    apply_startup_resume, cancel_restore, poll_restore, request_restore_thread,
};
#[cfg(test)]
pub(super) use loading::{finish_restore_for_test, restore_latest_thread, restore_thread_by_id};

fn apply_prepared_restore(
    prepared: loading::PreparedRestore,
    app: &mut TuiApp,
    agent: &mut Agent,
) -> Result<()> {
    let loading::PreparedRestore {
        thread,
        todo_state,
        runtime_state,
        turns,
        live_entries,
        live_recovery_warning,
        latest_plan_lifecycle,
        goal,
    } = prepared;
    let thread_id = thread.metadata.session_id.clone();
    let mut resume_notice = format!("Resumed thread {thread_id}.");
    let mut resume_level = NoticeLevel::Info;
    let restored_goal = match goal {
        Ok(prepared) => app.goal_handle.apply_prepared_restore(prepared),
        Err(reason) => {
            log::warn!("Goal persistence unavailable for resumed thread {thread_id}: {reason}");
            app.goal_handle
                .disable_after_persistence_failure(reason.clone());
            resume_notice.push_str(&format!(" Goal persistence unavailable: {reason}"));
            resume_level = NoticeLevel::Warning;
            None
        }
    };
    let crate::thread_store::ThreadSnapshot {
        metadata,
        provenance: _,
        history,
        compaction,
        plan_explanation,
        plan_steps,
        interactions,
        rollout_items: _,
    } = thread;
    agent.history = history;
    agent.session_id = metadata.session_id;
    agent.todo_state = todo_state;
    if let Some(runtime_state) = runtime_state {
        let approval_mode = match parse_bash_approval_mode(&runtime_state.bash_approval) {
            Some(mode) => mode,
            None => {
                let warning = "Unknown saved bash approval mode; restored suggestion mode.";
                log::warn!("{warning}");
                resume_notice.push(' ');
                resume_notice.push_str(warning);
                resume_level = NoticeLevel::Warning;
                BashApprovalMode::Suggestion
            }
        };
        agent.set_bash_approval_mode(approval_mode);
        let mut prompt_config = agent.prompt_config().clone();
        prompt_config.append_system_prompt = runtime_state.prompt_runtime.append_system_prompt;
        prompt_config.warnings = runtime_state.prompt_runtime.warnings;
        agent.set_prompt_config(prompt_config);
    }
    apply_compaction_record(agent, &compaction);
    agent.compact_state.last_compaction_boundary = match compaction.boundary_version {
        Some(version) => Some(CompactBoundaryMetadata {
            version,
            before_tokens: compaction.before_tokens.unwrap_or_default(),
            recent_file_count: compaction.recent_file_count.unwrap_or_default(),
        }),
        None => latest_compact_boundary_metadata(&agent.history),
    };
    if !plan_steps.is_empty() {
        agent.current_plan = plan_steps
            .into_iter()
            .map(|step| PlanStep {
                step: step.step,
                status: match step.status.as_str() {
                    "completed" => PlanStepStatus::Completed,
                    "in_progress" => PlanStepStatus::InProgress,
                    _ => PlanStepStatus::Pending,
                },
            })
            .collect();
    } else {
        agent.current_plan.clear();
    }
    agent.plan_explanation = plan_explanation;
    agent.pending_user_input = None;
    agent.pending_approval = None;
    agent.completed_user_input = None;
    agent.completed_approval = None;
    for interaction in interactions {
        match (interaction.kind.as_str(), interaction.status.as_str()) {
            ("request_input", "pending") => {
                let Some(payload) = interaction.payload.as_ref() else {
                    continue;
                };
                let options = payload
                    .get("options")
                    .and_then(|value| value.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| {
                                let pair = item.as_array()?;
                                let label = pair.first()?.as_str()?.to_string();
                                let detail = pair.get(1)?.as_str()?.to_string();
                                Some((label, detail))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                agent.pending_user_input = Some(PendingUserInput {
                    question: payload
                        .get("question")
                        .and_then(|value| value.as_str())
                        .unwrap_or(&interaction.title)
                        .to_string(),
                    options,
                    note: payload
                        .get("note")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                });
            }
            ("approval", "pending") => {
                let payload = interaction.payload.as_ref();
                let command = payload
                    .and_then(|payload| payload.get("command"))
                    .and_then(|value| value.as_str())
                    .unwrap_or(&interaction.summary)
                    .to_string();
                let request = payload
                    .cloned()
                    .map(BashCommandInput::from_value)
                    .transpose()
                    .unwrap_or(None)
                    .unwrap_or(BashCommandInput {
                        command: Some(command.clone()),
                        program: None,
                        args: Vec::new(),
                        cwd: None,
                        env: Default::default(),
                        allow_net: payload
                            .and_then(|payload| payload.get("allow_net"))
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        run_in_background: payload
                            .and_then(|payload| payload.get("run_in_background"))
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        ..Default::default()
                    });
                agent.pending_approval = Some(PendingApproval {
                    tool_use_id: payload
                        .and_then(|payload| payload.get("tool_use_id"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("restored")
                        .to_string(),
                    request,
                });
            }
            ("request_input", "completed") => {
                agent.completed_user_input = Some(CompletedInteraction {
                    title: interaction.title,
                    summary: interaction.summary,
                });
            }
            ("approval", "completed") => {
                agent.completed_approval = Some(CompletedInteraction {
                    title: interaction.title,
                    summary: interaction.summary,
                });
            }
            _ => {}
        }
    }
    // Session-local interactions must not be carried into the new snapshot.
    // This is a presentation reset; the old session was flushed before loading.
    app.snapshot.pending_interactions.clear();
    app.snapshot.completed_interactions.clear();
    app.bottom_pane.pending_planning_suggestion = None.into();
    app.bottom_pane.pending_follow_up_messages.clear();
    app.bottom_pane.queued_follow_up_messages.clear();
    app.running_tool_boundary_count = 0;
    app.restore_committed_turns(turns);
    app.active_turn.entries = live_entries;
    if let Some(warning) = live_recovery_warning {
        resume_notice.push(' ');
        resume_notice.push_str(&warning);
        resume_level = NoticeLevel::Warning;
    }
    app.apply_runtime_snapshot(
        agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(agent, 0),
    );
    let pending_plan_tool_id = latest_plan_lifecycle
        .as_ref()
        .filter(|(phase, _)| phase == "plan_ready")
        .and_then(|(_, tool_use_id)| tool_use_id.as_deref());
    if latest_plan_lifecycle
        .as_ref()
        .is_some_and(|(phase, _)| phase == "plan_ready")
    {
        agent.set_execution_mode(AgentExecutionMode::Plan);
        if let Some(tool_id) = pending_plan_tool_id {
            agent.restore_pending_plan_exit_approval(tool_id);
        }
        app.apply_runtime_snapshot(
            agent,
            crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(agent, 0),
        );
        app.show_pending_plan_approval(pending_plan_tool_id);
        app.set_runtime_phase(
            super::state::RuntimePhase::Idle,
            Some("awaiting plan approval".into()),
        );
    }

    app.goal = restored_goal;

    app.push_notice(resume_level, resume_notice);
    super::goal_resume::arm_after_restore(app);
    Ok(())
}

fn apply_compaction_record(agent: &mut Agent, compaction: &CompactionRecord) {
    agent.compact_state.compaction_count = compaction.compaction_count;
    agent.compact_state.last_compaction_before_tokens = compaction.before_tokens;
    agent.compact_state.last_compaction_after_tokens = compaction.after_tokens;
}

fn parse_bash_approval_mode(mode: &str) -> Option<BashApprovalMode> {
    match mode {
        "once" => Some(BashApprovalMode::Once),
        "always" => Some(BashApprovalMode::Always),
        "suggestion" => Some(BashApprovalMode::Suggestion),
        _ => None,
    }
}

pub(crate) fn provider_requires_api_key(provider: &str) -> bool {
    !matches!(
        provider,
        "mock" | "local" | "local-candle" | "gemma4" | "qwen3" | "qwn3" | "ollama" | "bedrock"
    )
}

#[cfg(test)]
#[path = "session_restore_goal_tests.rs"]
mod goal_tests;

#[cfg(test)]
mod tests;
