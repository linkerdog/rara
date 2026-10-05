use std::fs;

use rara_memory::memory_handle::MemoryHandle;
use rara_state::state_db::{PersistedTurnEntry, StateDb};
use rara_tools::tool::ToolManager;
use serde_json::json;
use tempfile::tempdir;

use super::*;
use crate::agent::{AgentExecutionMode, Message};
use crate::config::ConfigManager;
use crate::llm::MockLlm;
use crate::prompt::PromptRuntimeConfig;
use crate::todo::{TodoItem, TodoState, TodoStatus};
use crate::tui::state::{ActivePendingInteractionKind, InteractionKind, TuiApp};
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
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
            active_form: None,
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
    original_app.apply_runtime_snapshot(
        &original_agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&original_agent, 0),
    );

    original_agent.execution_mode = AgentExecutionMode::Execute;
    let expected_runtime = original_agent.shared_runtime_context();

    let restored_agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
    original_app.apply_runtime_snapshot(
        &original_agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&original_agent, 0),
    );

    let rollout_dir = rara_dir
        .join("rollouts")
        .join(original_agent.session_id.as_str());
    if rollout_dir.exists() {
        fs::remove_dir_all(&rollout_dir).expect("remove rollout history");
    }

    let restored_agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
    restored_app.bottom_pane.pending_planning_suggestion =
        Some("stale planning suggestion".to_string()).into();
    restored_app.queue_follow_up_message("stale queued follow-up");

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
fn restore_session_recovers_live_active_turn_entries() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("repo");
    let rara_dir = root.join(".rara");
    fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let session_manager = Arc::new(crate::session::SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    let workspace = Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir.clone()));
    let backend = Arc::new(MockLlm);
    let mut original_agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager.clone(),
        workspace.clone(),
    );
    original_agent.session_id = "session-live-active-turn".to_string();
    original_agent.history.push(Message {
        role: "user".to_string(),
        content: json!([{"type":"text","text":"recover active turn"}]),
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
    original_app.apply_runtime_snapshot(
        &original_agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&original_agent, 0),
    );
    rara_persistence::thread_turn_log::append_rollout_fragment(
        &state_db.rollout_root(),
        &original_agent.session_id,
        &PersistedTurnEntry {
            role: "You".to_string(),
            message: "recover active turn".to_string(),
        },
    )
    .expect("write live user entry");
    rara_persistence::thread_turn_log::append_rollout_fragment(
        &state_db.rollout_root(),
        &original_agent.session_id,
        &PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "partial answer".to_string(),
        },
    )
    .expect("write live agent entry");
    for (role, message) in [
        ("Tool Result", "bash finished with exit code 0"),
        ("legacy\x1b[31m-note\x1b[0m", "historical annotation"),
        ("Agent Delta", "historical event label"),
    ] {
        rara_persistence::thread_turn_log::append_rollout_fragment(
            &state_db.rollout_root(),
            &original_agent.session_id,
            &PersistedTurnEntry {
                role: role.into(),
                message: message.into(),
            },
        )
        .expect("write historical transcript entry");
    }

    let restored_agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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

    assert_eq!(restored_app.committed_turns.len(), 0);
    assert_eq!(restored_app.active_turn.entries.len(), 6);
    assert_eq!(
        restored_app.active_turn.entries[5].role,
        MessageRole::System
    );
    assert_eq!(
        Some(restored_app.active_turn.entries[5].message.as_str()),
        restored_app.notice_text()
    );
    assert!(
        restored_app
            .bottom_pane
            .pending_planning_suggestion
            .is_none()
    );
    assert!(restored_app.pop_queued_follow_up_message().is_none());
    assert_eq!(restored_app.active_turn.entries[0].role, MessageRole::User);
    assert_eq!(
        restored_app.active_turn.entries[0].message,
        "recover active turn"
    );
    assert_eq!(restored_app.active_turn.entries[1].role, MessageRole::Agent);
    assert_eq!(
        restored_app.active_turn.entries[1].message,
        "partial answer"
    );
    let entries = &restored_app.active_turn.entries;
    assert_eq!(entries[2].role, MessageRole::ToolResult);
    assert_eq!(entries[3].role, MessageRole::Legacy("legacy-note".into()));
    assert_eq!(entries[4].role, MessageRole::Legacy("Agent Delta".into()));
    let rendered =
        crate::tui::render::prefixed_message_lines(&entries[3].role, &entries[3].message, 4)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
    assert_eq!(rendered, "legacy-note: historical annotation");
    assert!(!restored_app.has_agent_stream());
    assert!(!restored_app.has_agent_thinking_stream());

    // Commit recovered live entries, then exercise committed restoration as well.
    restored_app.finalize_active_turn();
    let state_db = restored_app.state_db.as_ref().expect("state db");
    let stored = rara_persistence::thread_turn_log::load_turn_records(
        &state_db.rollout_root(),
        &original_agent.session_id,
    )
    .expect("load committed turn");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0]
            .entries
            .iter()
            .map(|entry| entry.role.as_str())
            .collect::<Vec<_>>(),
        [
            "You",
            "Agent",
            "Tool Result",
            "legacy-note",
            "Agent Delta",
            "System"
        ],
    );
    restore_thread_by_id(
        &original_agent.session_id,
        &mut restored_app,
        &mut restored_slot,
    )
    .expect("restore committed turn");
    assert_eq!(restored_app.active_turn.entries.len(), 1);
    assert_eq!(
        restored_app.active_turn.entries[0].role,
        MessageRole::System
    );
    let entries = &restored_app.committed_turns[0].entries;
    assert_eq!(entries.len(), 6);
    assert_eq!(entries[5].role, MessageRole::System);
    assert_eq!(entries[0].role, MessageRole::User);
    assert_eq!(entries[1].role, MessageRole::Agent);
    assert_eq!(entries[2].role, MessageRole::ToolResult);
    assert_eq!(entries[3].role, MessageRole::Legacy("legacy-note".into()));
    assert_eq!(entries[4].role, MessageRole::Legacy("Agent Delta".into()));
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
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
    original_app.apply_runtime_snapshot(
        &original_agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&original_agent, 0),
    );

    let restored_agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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

#[test]
fn restore_session_recovers_pending_plan_approval_from_lifecycle() {
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
    let state_db = Arc::new(StateDb::new_for_root_dir(rara_dir.clone()).expect("state db"));

    let mut original_agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager.clone(),
        workspace.clone(),
    );
    original_agent.session_id = "session-pending-plan-approval".to_string();
    original_agent.set_execution_mode(AgentExecutionMode::Plan);
    original_agent.current_plan = vec![PlanStep {
        step: "Restore plan approval".to_string(),
        status: PlanStepStatus::Pending,
    }];
    original_agent.plan_explanation = Some("Recover the pending approval card.".to_string());
    original_agent.history.push(Message {
        role: "user".to_string(),
        content: json!([{"type":"text","text":"resume approval"}]),
    });
    session_manager
        .save_session(&original_agent.session_id, &original_agent.history)
        .expect("save session");

    let mut original_app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    original_app.attach_state_db(state_db.clone());
    original_app.apply_runtime_snapshot(
        &original_agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&original_agent, 0),
    );
    original_app.show_pending_plan_approval(Some("exit-plan-restore"));

    let restored_agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
    assert_eq!(restored_agent.execution_mode, AgentExecutionMode::Plan);
    assert!(restored_agent.has_pending_plan_exit_approval());
    assert_eq!(
        restored_agent.pending_plan_exit_tool_id(),
        Some("exit-plan-restore")
    );
    assert!(restored_app.has_pending_plan_approval());
    assert_eq!(
        restored_app
            .active_pending_interaction()
            .map(|interaction| interaction.kind),
        Some(ActivePendingInteractionKind::PlanApproval)
    );
    assert_eq!(
        restored_app
            .pending_plan_approval_interaction()
            .and_then(|interaction| interaction.source.as_deref()),
        Some("exit_plan_mode:exit-plan-restore")
    );
}

#[test]
fn restore_session_does_not_reopen_completed_plan_approval() {
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
    let state_db = Arc::new(StateDb::new_for_root_dir(rara_dir.clone()).expect("state db"));

    let mut original_agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager.clone(),
        workspace.clone(),
    );
    original_agent.session_id = "session-completed-plan-approval".to_string();
    original_agent.set_execution_mode(AgentExecutionMode::Plan);
    original_agent.current_plan = vec![PlanStep {
        step: "Do not reopen approval".to_string(),
        status: PlanStepStatus::Pending,
    }];
    original_agent.plan_explanation = Some("Completed approvals stay completed.".to_string());
    original_agent.history.push(Message {
        role: "user".to_string(),
        content: json!([{"type":"text","text":"resume completed approval"}]),
    });
    session_manager
        .save_session(&original_agent.session_id, &original_agent.history)
        .expect("save session");

    let mut original_app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    original_app.attach_state_db(state_db.clone());
    original_app.apply_runtime_snapshot(
        &original_agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&original_agent, 0),
    );
    original_app.show_pending_plan_approval(Some("exit-plan-completed"));
    original_app.clear_pending_plan_approval();
    original_app.record_completed_interaction(
        InteractionKind::PlanApproval,
        "Plan Decision",
        "copy can change",
        Some("plan_approval:approve".to_string()),
    );

    let restored_agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
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
    assert!(!restored_agent.has_pending_plan_exit_approval());
    assert!(!restored_app.has_pending_plan_approval());
}
