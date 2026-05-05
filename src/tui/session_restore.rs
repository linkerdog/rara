use std::sync::Arc;

use anyhow::Result;

use super::state::{TranscriptEntry, TranscriptTurn, TuiApp};
use crate::agent::{
    Agent, BashApprovalMode, CompactBoundaryMetadata, CompletedInteraction, PendingApproval,
    PendingUserInput, PlanStep, PlanStepStatus, latest_compact_boundary_metadata,
};
use crate::state_db::StateDb;
use crate::thread_store::{CompactionRecord, RolloutItem, ThreadStore};
use crate::tools::bash::BashCommandInput;

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
    agent.todo_state = agent.session_manager.load_todo_state(thread_id)?;
    if let Some(runtime_state) = state_db.load_session_runtime_state(thread_id)? {
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
                    .map(|entry| TranscriptEntry::new(entry.role, entry.message))
                    .collect::<Vec<_>>();
                turns.push(TranscriptTurn { entries });
            }
            RolloutItem::Turn(_)
            | RolloutItem::Compaction(_)
            | RolloutItem::PlanState { .. }
            | RolloutItem::Interaction(_) => {}
        }
    }
    if !turns.is_empty() {
        app.restore_committed_turns(turns);
    } else {
        app.reset_transcript();
    }
    app.sync_snapshot(agent);
    app.notice = Some(format!("Resumed thread {thread_id}."));
    Ok(())
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
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::agent::{AgentExecutionMode, Message};
    use crate::config::ConfigManager;
    use crate::llm::MockLlm;
    use crate::prompt::PromptRuntimeConfig;
    use crate::state_db::StateDb;
    use crate::todo::{TodoItem, TodoState, TodoStatus};
    use crate::tool::ToolManager;
    use crate::tui::state::TuiApp;
    use crate::vectordb::VectorDB;
    use crate::workspace::WorkspaceMemory;

    #[test]
    fn restore_session_keeps_runtime_context_and_snapshot_aligned() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let rara_dir = root.join(".rara");
        fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
        fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
        fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");
        fs::write(root.join("AGENTS.md"), "repo rules").expect("agents");

        let session_manager = Arc::new(crate::session::SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        });
        let workspace = Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir.clone()));
        let backend = Arc::new(MockLlm);

        let mut original_agent = Agent::new(
            ToolManager::new(),
            backend.clone(),
            Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager.clone(),
            workspace.clone(),
        );
        original_agent.session_id = "session-restore-1".to_string();
        original_agent.execution_mode = AgentExecutionMode::Plan;
        original_agent.set_prompt_config(PromptRuntimeConfig {
            append_system_prompt: Some("appendix".to_string()),
            warnings: vec!["missing custom prompt file".to_string()],
            ..PromptRuntimeConfig::default()
        });
        original_agent.current_plan = vec![PlanStep {
            step: "Align runtime restore with shared context".to_string(),
            status: PlanStepStatus::Pending,
        }];
        original_agent.plan_explanation =
            Some("Restore should rebuild the same context surface.".to_string());
        original_agent.todo_state = Some(TodoState {
            version: 1,
            updated_at: 42,
            items: vec![TodoItem {
                id: "todo-1".to_string(),
                content: "Restore todo state".to_string(),
                status: TodoStatus::InProgress,
                updated_at: 42,
            }],
        });
        session_manager
            .save_todo_state(
                &original_agent.session_id,
                original_agent.todo_state.as_ref().expect("todo state"),
            )
            .expect("save todo state");
        original_agent.compact_state.compaction_count = 1;
        original_agent.compact_state.last_compaction_before_tokens = Some(2400);
        original_agent.compact_state.last_compaction_after_tokens = Some(900);
        original_agent.compact_state.last_compaction_boundary = Some(CompactBoundaryMetadata {
            version: 2,
            before_tokens: 2400,
            recent_file_count: 3,
        });
        original_agent.history.push(Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"resume me"}]),
        });
        session_manager
            .save_session(&original_agent.session_id, &original_agent.history)
            .expect("save session");

        let state_db = Arc::new(StateDb::new_for_root_dir(rara_dir.clone()).expect("state db"));
        let mut original_app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        original_app.attach_state_db(state_db.clone());
        original_app.sync_snapshot(&original_agent);

        original_agent.execution_mode = AgentExecutionMode::Execute;
        let expected_runtime = original_agent.shared_runtime_context();

        let restored_agent = Agent::new(
            ToolManager::new(),
            backend,
            Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager,
            workspace,
        );
        let mut restored_slot = Some(restored_agent);
        let mut restored_app = TuiApp::new(ConfigManager {
            path: temp.path().join("config-restored.json"),
        })
        .expect("restored app");
        restored_app.attach_state_db(state_db);

        restore_thread_by_id(
            expected_runtime.session_id.as_str(),
            &mut restored_app,
            &mut restored_slot,
        )
        .expect("restore thread");

        let restored_agent = restored_slot.expect("restored agent");
        let restored_runtime = restored_agent.shared_runtime_context();

        assert_eq!(restored_agent.execution_mode, AgentExecutionMode::Execute);
        assert_eq!(restored_runtime.cwd, expected_runtime.cwd);
        assert_eq!(restored_runtime.branch, expected_runtime.branch);
        assert_eq!(restored_runtime.session_id, expected_runtime.session_id);
        assert_eq!(restored_runtime.history_len, expected_runtime.history_len);
        assert_eq!(
            restored_runtime.prompt.base_prompt_kind,
            expected_runtime.prompt.base_prompt_kind
        );
        assert_eq!(
            restored_runtime.prompt.section_keys,
            expected_runtime.prompt.section_keys
        );
        assert_eq!(
            restored_runtime.prompt.source_entries,
            expected_runtime.prompt.source_entries
        );
        assert_eq!(
            restored_runtime.prompt.append_system_prompt,
            expected_runtime.prompt.append_system_prompt
        );
        assert_eq!(
            restored_runtime.prompt.warnings,
            expected_runtime.prompt.warnings
        );
        assert_eq!(restored_runtime.plan.steps, expected_runtime.plan.steps);
        assert_eq!(
            restored_runtime.plan.explanation,
            expected_runtime.plan.explanation
        );
        assert_eq!(restored_runtime.todo, expected_runtime.todo);
        assert_eq!(
            restored_runtime.assembly.entries,
            expected_runtime.assembly.entries
        );
        assert_eq!(
            restored_runtime.compaction.last_compaction_boundary_version,
            expected_runtime.compaction.last_compaction_boundary_version
        );

        assert_eq!(
            restored_app.snapshot.prompt_source_entries,
            restored_runtime.prompt.source_entries
        );
        assert_eq!(
            restored_app.snapshot.prompt_append_system_prompt,
            restored_runtime.prompt.append_system_prompt
        );
        assert_eq!(
            restored_app.snapshot.plan_steps,
            restored_runtime.plan.steps
        );
        assert_eq!(
            restored_app.snapshot.plan_explanation,
            restored_runtime.plan.explanation
        );
        assert_eq!(
            restored_app.snapshot.assembly_entries,
            restored_runtime.assembly.entries
        );
        assert_eq!(
            restored_app.snapshot.last_compaction_boundary_version,
            restored_runtime.compaction.last_compaction_boundary_version
        );
    }

    #[test]
    fn restore_session_keeps_target_session_id_even_without_history_file() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let rara_dir = root.join(".rara");
        fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
        fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
        fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");
        fs::write(root.join("AGENTS.md"), "repo rules").expect("agents");

        let session_manager = Arc::new(crate::session::SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        });
        let workspace = Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir.clone()));
        let backend = Arc::new(MockLlm);

        let mut original_agent = Agent::new(
            ToolManager::new(),
            backend.clone(),
            Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager.clone(),
            workspace.clone(),
        );
        original_agent.session_id = "session-without-history".to_string();
        original_agent.history.push(Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"restore this exact session"}]),
        });

        let state_db = Arc::new(StateDb::new_for_root_dir(rara_dir.clone()).expect("state db"));
        let mut original_app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        original_app.attach_state_db(state_db.clone());
        original_app.sync_snapshot(&original_agent);

        let rollout_dir = rara_dir
            .join("rollouts")
            .join(original_agent.session_id.as_str());
        if rollout_dir.exists() {
            fs::remove_dir_all(&rollout_dir).expect("remove rollout history");
        }

        let restored_agent = Agent::new(
            ToolManager::new(),
            backend,
            Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager,
            workspace,
        );
        let mut restored_slot = Some(restored_agent);
        let mut restored_app = TuiApp::new(ConfigManager {
            path: temp.path().join("config-restored.json"),
        })
        .expect("restored app");
        restored_app.attach_state_db(state_db);

        restore_thread_by_id(
            original_agent.session_id.as_str(),
            &mut restored_app,
            &mut restored_slot,
        )
        .expect("restore thread");

        let restored_agent = restored_slot.expect("restored agent");
        assert_eq!(restored_agent.session_id, "session-without-history");
        assert_eq!(restored_app.snapshot.session_id, "session-without-history");
    }

    #[test]
    fn restore_session_surfaces_pending_interactions_in_assembled_context() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let rara_dir = root.join(".rara");
        fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
        fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
        fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");
        fs::write(root.join("AGENTS.md"), "repo rules").expect("agents");

        let session_manager = Arc::new(crate::session::SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        });
        let workspace = Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir.clone()));
        let backend = Arc::new(MockLlm);

        let mut original_agent = Agent::new(
            ToolManager::new(),
            backend.clone(),
            Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager.clone(),
            workspace.clone(),
        );
        original_agent.session_id = "session-pending-context".to_string();
        original_agent.current_plan = vec![PlanStep {
            step: "Restore pending approval".to_string(),
            status: PlanStepStatus::Pending,
        }];
        original_agent.plan_explanation = Some("Keep restore and context aligned.".to_string());
        original_agent.compact_state.compaction_count = 1;
        original_agent.compact_state.last_compaction_before_tokens = Some(1800);
        original_agent.compact_state.last_compaction_after_tokens = Some(900);
        original_agent.pending_user_input = Some(PendingUserInput {
            question: "Which path should we keep?".to_string(),
            options: vec![("1".to_string(), "shared".to_string())],
            note: Some("Need the user's decision before continuing.".to_string()),
        });
        original_agent.pending_approval = Some(PendingApproval {
            tool_use_id: "tool-approval-1".to_string(),
            request: BashCommandInput {
                command: Some("cargo test".to_string()),
                program: Some("cargo".to_string()),
                args: vec!["test".to_string()],
                cwd: Some(root.display().to_string()),
                env: Default::default(),
                allow_net: false,
                run_in_background: false,
                ..Default::default()
            },
        });
        original_agent.history.push(Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"resume the blocked thread"}]),
        });
        session_manager
            .save_session(&original_agent.session_id, &original_agent.history)
            .expect("save session");

        let state_db = Arc::new(StateDb::new_for_root_dir(rara_dir.clone()).expect("state db"));
        let mut original_app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        original_app.attach_state_db(state_db.clone());
        original_app.sync_snapshot(&original_agent);

        let restored_agent = Agent::new(
            ToolManager::new(),
            backend,
            Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager,
            workspace,
        );
        let mut restored_slot = Some(restored_agent);
        let mut restored_app = TuiApp::new(ConfigManager {
            path: temp.path().join("config-restored.json"),
        })
        .expect("restored app");
        restored_app.attach_state_db(state_db);

        restore_thread_by_id(
            original_agent.session_id.as_str(),
            &mut restored_app,
            &mut restored_slot,
        )
        .expect("restore thread");

        let restored_agent = restored_slot.expect("restored agent");
        let runtime = restored_agent.shared_runtime_context();
        assert!(
            runtime
                .assembly
                .entries
                .iter()
                .any(|entry| entry.layer == "active_turn_state"
                    && entry.kind == "request_input"
                    && entry.injected)
        );
        assert!(
            runtime
                .assembly
                .entries
                .iter()
                .any(|entry| entry.layer == "active_turn_state"
                    && entry.kind == "approval"
                    && entry.injected)
        );
        assert_eq!(
            restored_app.snapshot.assembly_entries,
            runtime.assembly.entries
        );
    }
}
