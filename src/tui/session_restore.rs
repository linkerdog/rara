use std::sync::Arc;

use anyhow::Result;
use rara_state::state_db::StateDb;

use super::state::{TranscriptEntry, TranscriptTurn, TuiApp};
use crate::agent::{
    Agent, AgentExecutionMode, BashApprovalMode, CompactBoundaryMetadata, CompletedInteraction,
    PendingApproval, PendingUserInput, PlanStep, PlanStepStatus, latest_compact_boundary_metadata,
};
use crate::thread_store::{CompactionRecord, RolloutItem, ThreadStore};
use crate::tools::bash::BashCommandInput;
use crate::tui::message_role::MessageRole;

pub(super) fn restore_latest_thread(
    state_db: &Arc<StateDb>,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
) -> Result<()> {
    let Some(agent) = agent_slot.as_ref() else {
        return Ok(());
    };
    let store = ThreadStore::new(agent.session_manager.as_ref(), state_db.as_ref());
    let Some(thread) = store.latest_thread_summary()? else {
        return Ok(());
    };
    restore_thread_by_id(thread.metadata.session_id.as_str(), app, agent_slot)
}

pub(super) fn restore_thread_by_id(
    thread_id: &str,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
) -> Result<()> {
    let Some(agent) = agent_slot.as_mut() else {
        return Ok(());
    };
    let Some(state_db) = app.state_db.as_ref() else {
        return Ok(());
    };
    let thread_store = ThreadStore::new(agent.session_manager.as_ref(), state_db.as_ref());
    let thread = thread_store.load_thread(thread_id)?;
    let todo_state = agent.session_manager.load_todo_state(thread_id)?;
    let runtime_state = state_db.load_session_runtime_state(thread_id)?;
    // Required thread reads succeed before rebinding optional goal state.
    let mut resume_notice = format!("Resumed thread {thread_id}.");
    let restored_goal = match app
        .goal_handle
        .restore_for_thread(thread_id, state_db.clone())
    {
        Ok(goal) => goal,
        Err(error) => {
            let reason = format!("{error:#}");
            log::warn!("Goal persistence unavailable for resumed thread {thread_id}: {reason}");
            app.goal_handle
                .disable_after_persistence_failure(reason.clone());
            resume_notice.push_str(&format!(" Goal persistence unavailable: {reason}"));
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
        rollout_items,
    } = thread;
    agent.history = history;
    agent.session_id = metadata.session_id;
    agent.todo_state = todo_state;
    if let Some(runtime_state) = runtime_state {
        agent.set_bash_approval_mode(parse_bash_approval_mode(
            runtime_state.bash_approval.as_str(),
        ));
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
    let latest_plan_lifecycle = latest_plan_lifecycle(&rollout_items);
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
    let mut turns = Vec::new();
    for item in rollout_items {
        match item {
            RolloutItem::Turn(turn) if !turn.entries.is_empty() => {
                let entries = turn
                    .entries
                    .into_iter()
                    .map(|entry| {
                        TranscriptEntry::new(
                            MessageRole::from_persisted(&entry.role),
                            entry.message,
                        )
                    })
                    .collect::<Vec<_>>();
                turns.push(TranscriptTurn {
                    thinking_duration: None,
                    entries,
                });
            }
            RolloutItem::Turn(_)
            | RolloutItem::Compaction(_)
            | RolloutItem::PlanState { .. }
            | RolloutItem::Interaction(_)
            | RolloutItem::PlanLifecycle(_)
            | RolloutItem::SpawnAgent { .. } => {}
        }
    }
    let rollout_root = state_db.rollout_root();
    if !turns.is_empty() {
        app.restore_committed_turns(turns);
    } else {
        app.reset_transcript();
    }
    let live_entries =
        rara_persistence::thread_turn_log::load_live_entries(&rollout_root, thread_id);
    if !live_entries.is_empty() {
        app.active_turn.entries = live_entries
            .into_iter()
            .map(|entry| {
                TranscriptEntry::new(MessageRole::from_persisted(&entry.role), entry.message)
            })
            .collect();
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

    app.bottom_pane.notice = Some(resume_notice);
    Ok(())
}

fn latest_plan_lifecycle(rollout_items: &[RolloutItem]) -> Option<(String, Option<String>)> {
    rollout_items.iter().rev().find_map(|item| match item {
        RolloutItem::PlanLifecycle(lifecycle) => {
            Some((lifecycle.phase.clone(), lifecycle.tool_use_id.clone()))
        }
        _ => None,
    })
}

fn apply_compaction_record(agent: &mut Agent, compaction: &CompactionRecord) {
    agent.compact_state.compaction_count = compaction.compaction_count;
    agent.compact_state.last_compaction_before_tokens = compaction.before_tokens;
    agent.compact_state.last_compaction_after_tokens = compaction.after_tokens;
}

fn parse_bash_approval_mode(mode: &str) -> BashApprovalMode {
    match mode {
        "once" => BashApprovalMode::Once,
        "suggestion" => BashApprovalMode::Suggestion,
        _ => BashApprovalMode::Always,
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
