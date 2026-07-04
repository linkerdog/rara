use std::fs;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use rara_memory::vectordb::VectorDB;
use rara_persistence::thread_data::{
    PersistedCompactState, PersistedInteraction, PersistedPlanLifecycle, PersistedPlanStep,
    PersistedPromptRuntimeState, PersistedRuntimeRolloutItem, PersistedStructuredRolloutEvent,
    PersistedThreadLineage, PersistedThreadRecord, PersistedTurnEntry,
};
use rara_persistence::thread_metadata;
use rara_persistence::{thread_rollout_log, thread_turn_log};
use rara_state::state_db::StateDb;
use serde_json::Value;
use tempfile::tempdir;

use super::{
    RolloutItem, ThreadHistorySource, ThreadMetadataSource, ThreadNonTurnRolloutSource,
    ThreadRecorder, ThreadRuntimeLineage, ThreadRuntimeState, ThreadStore,
};
use crate::agent::Message;
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, MockLlm, TokenUsage};
use crate::memory_store::{MemoryLabel, MemoryScope, MemorySource, MemoryStore};
use crate::session::{PersistedCompactionEvent, SessionManager};

#[test]
fn load_thread_aggregates_history_state_and_rollout_items() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    session_manager.save_session(
        "session-1",
        &[Message {
            role: "user".to_string(),
            content: serde_json::json!("hello"),
        }],
    )?;
    state_db.upsert_session(
        "session-1",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        Some("Inspect current plan state."),
        &PersistedPromptRuntimeState::default(),
        1,
        1,
        &PersistedCompactState {
            compaction_count: 2,
            last_compaction_before_tokens: Some(8000),
            last_compaction_after_tokens: Some(2400),
            last_compaction_recent_file_count: Some(1),
            last_compaction_boundary_version: Some(3),
        },
    )?;
    state_db.replace_plan_steps(
        "session-1",
        &[PersistedPlanStep {
            step_index: 0,
            status: "in_progress".to_string(),
            step: "Inspect src/thread_store.rs".to_string(),
        }],
    )?;
    state_db.replace_interactions(
        "session-1",
        &[PersistedInteraction {
            kind: "approval".to_string(),
            status: "pending".to_string(),
            title: "Need approval".to_string(),
            summary: "cargo check".to_string(),
            payload: None,
        }],
    )?;
    state_db.persist_turn(
        "session-1",
        0,
        &[PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "Investigating.".to_string(),
        }],
    )?;
    session_manager.save_compaction_event(
        "session-1",
        &PersistedCompactionEvent {
            event_index: 2,
            before_tokens: 8000,
            after_tokens: 2400,
            boundary_version: 3,
            replaced_start: None,
            replaced_end: None,
            metadata_owner: None,
            recent_files: vec!["src/thread_store.rs".to_string()],
            summary: "Compacted earlier repository inspection.".to_string(),
        },
    )?;
    session_manager.save_spawn_agent_event(
        "session-1",
        "spawn-1",
        "worker-1",
        Some("Worker 1"),
        "child-session-1",
        "done",
        Some("Child completed."),
        Some(4096),
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-1")?;
    let spawn_edges = state_db.load_spawn_agent_edges("session-1")?;

    assert_eq!(snapshot.metadata.session_id, "session-1");
    assert_eq!(spawn_edges.len(), 1);
    assert_eq!(spawn_edges[0].agent_id, "worker-1");
    assert_eq!(spawn_edges[0].child_session_id, "child-session-1");
    assert_eq!(spawn_edges[0].token_budget, Some(4096));
    assert_eq!(
        snapshot.provenance.metadata_source,
        ThreadMetadataSource::StateDb
    );
    assert_eq!(
        snapshot.provenance.history_source,
        ThreadHistorySource::CanonicalHistory
    );
    assert_eq!(
        snapshot.provenance.non_turn_rollout_source,
        ThreadNonTurnRolloutSource::StructuredEventsLog
    );
    assert_eq!(snapshot.metadata.provider, "ollama");
    assert_eq!(snapshot.history.len(), 1);
    assert_eq!(snapshot.compaction.compaction_count, 2);
    assert_eq!(
        snapshot.compaction.summary.as_deref(),
        Some("Compacted earlier repository inspection.")
    );
    assert_eq!(
        snapshot.compaction.recent_files,
        vec!["src/thread_store.rs".to_string()]
    );
    assert_eq!(
        snapshot.plan_explanation.as_deref(),
        Some("Inspect current plan state.")
    );
    assert_eq!(snapshot.plan_steps.len(), 1);
    assert_eq!(snapshot.interactions.len(), 1);
    assert_eq!(snapshot.rollout_items.len(), 5);
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::Compaction(compaction) if compaction.compaction_count == 2
    )));
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::PlanState { explanation, steps }
            if explanation.as_deref() == Some("Inspect current plan state.") && steps.len() == 1
    )));
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::Interaction(interaction)
            if interaction.kind == "approval" && interaction.status == "pending"
    )));
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::SpawnAgent {
            event_id,
            agent_id,
            name: Some(name),
            child_session_id,
            status,
            summary: Some(summary),
        } if event_id == "spawn-1"
            && agent_id == "worker-1"
            && name == "Worker 1"
            && child_session_id == "child-session-1"
            && status == "done"
            && summary == "Child completed."
    )));
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::Turn(turn)
            if turn.summary.ordinal == 0
                && turn.entries.len() == 1
                && turn.entries[0].message == "Investigating."
    )));

    Ok(())
}

#[test]
fn load_thread_materializes_from_roots_without_session_manager_facade() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let state_db = StateDb::new_for_root_dir(rara_dir.clone())?;
    let rollout_root = state_db.rollout_root();
    let legacy_session_root = rara_dir.join("sessions");
    fs::create_dir_all(&legacy_session_root)?;
    let history = vec![Message {
        role: "user".to_string(),
        content: serde_json::json!("load directly from thread roots"),
    }];
    crate::session_transcript::write_history_snapshot(
        &rollout_root,
        "session-root-store",
        &history,
    )?;
    state_db.upsert_session(
        "session-root-store",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        1,
        &PersistedCompactState::default(),
    )?;
    thread_rollout_log::append_rollout_event_line(
        &rollout_root,
        "session-root-store",
        &PersistedStructuredRolloutEvent::Compaction {
            recorded_at: None,
            event_index: 1,
            before_tokens: 4096,
            after_tokens: 1024,
            boundary_version: 2,
            replaced_start: Some(0),
            replaced_end: Some(1),
            metadata_owner: Some("runtime.compaction".to_string()),
            recent_files: vec!["src/thread_store.rs".to_string()],
            summary: "Compacted direct root materialization.".to_string(),
        },
    )?;

    let store = ThreadStore::new_for_roots(rollout_root, legacy_session_root, &state_db);
    let snapshot = store.load_thread("session-root-store")?;

    assert_eq!(
        snapshot.provenance.history_source,
        ThreadHistorySource::CanonicalHistory
    );
    assert_eq!(
        snapshot.provenance.non_turn_rollout_source,
        ThreadNonTurnRolloutSource::StructuredEventsLog
    );
    assert_eq!(snapshot.history, history);
    assert_eq!(snapshot.compaction.compaction_count, 1);
    assert_eq!(snapshot.compaction.replaced_start, Some(0));
    assert_eq!(snapshot.compaction.replaced_end, Some(1));
    assert_eq!(
        snapshot.compaction.metadata_owner.as_deref(),
        Some("runtime.compaction")
    );
    Ok(())
}

#[test]
fn load_thread_prefers_structured_metadata_without_state_db_row() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let state_db = StateDb::new_for_root_dir(rara_dir.clone())?;
    let rollout_root = state_db.rollout_root();
    let legacy_session_root = rara_dir.join("sessions");
    fs::create_dir_all(&legacy_session_root)?;
    let history = vec![Message {
        role: "user".to_string(),
        content: serde_json::json!("load metadata from thread.json"),
    }];
    crate::session_transcript::write_history_snapshot(
        &rollout_root,
        "session-structured-metadata",
        &history,
    )?;
    thread_metadata::write_thread_record(
        &rollout_root,
        &PersistedThreadRecord {
            session_id: "session-structured-metadata".to_string(),
            cwd: "/tmp/structured-workspace".to_string(),
            branch: "main".to_string(),
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
            base_url: None,
            agent_mode: "execute".to_string(),
            bash_approval: "suggest".to_string(),
            created_at: 10,
            lineage: PersistedThreadLineage::default(),
            plan_explanation: None,
            history_len: 1,
            transcript_len: 1,
            updated_at: 20,
        },
    )?;

    let store = ThreadStore::new_for_roots(rollout_root, legacy_session_root, &state_db);
    let snapshot = store.load_thread("session-structured-metadata")?;

    assert_eq!(
        snapshot.provenance.metadata_source,
        ThreadMetadataSource::StructuredMetadata
    );
    assert_eq!(snapshot.metadata.cwd, "/tmp/structured-workspace");
    assert_eq!(snapshot.history, history);
    Ok(())
}

#[test]
fn load_thread_keeps_session_without_history_file() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    state_db.upsert_session(
        "session-missing-history",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        None,
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-missing-history")?;

    assert_eq!(snapshot.metadata.session_id, "session-missing-history");
    assert!(snapshot.history.is_empty());
    assert_eq!(
        snapshot.provenance.history_source,
        ThreadHistorySource::Missing
    );
    Ok(())
}

#[test]
fn load_thread_rejects_history_without_thread_metadata() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    session_manager.save_session(
        "child-session",
        &[Message {
            role: "assistant".to_string(),
            content: serde_json::Value::String("Child result.".to_string()),
        }],
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let err = store
        .load_thread("child-session")
        .expect_err("history-only session should not fabricate metadata");

    assert!(
        err.to_string()
            .contains("Thread child-session not found in thread metadata")
    );
    Ok(())
}

#[test]
fn load_thread_backfills_legacy_history_file_into_rollout_root() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    state_db.upsert_session(
        "session-legacy-history",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        0,
        &PersistedCompactState::default(),
    )?;
    fs::write(
        session_manager
            .legacy_storage_dir
            .join("session-legacy-history.json"),
        serde_json::to_string(&vec![Message {
            role: "user".to_string(),
            content: serde_json::json!("legacy thread history"),
        }])?,
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-legacy-history")?;

    assert_eq!(snapshot.history.len(), 1);
    assert_eq!(
        snapshot.provenance.history_source,
        ThreadHistorySource::LegacyHistoryBackfilled
    );
    let canonical_history = fs::read_to_string(
        session_manager
            .storage_dir
            .join("session-legacy-history")
            .join("history.json"),
    )?;
    let canonical_messages: Vec<Message> = serde_json::from_str(&canonical_history)?;
    assert_eq!(canonical_messages.len(), 1);
    assert_eq!(canonical_messages[0].role, "user");
    Ok(())
}

#[test]
fn load_thread_prefers_structured_compaction_event_over_session_counters() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    state_db.upsert_session(
        "session-compaction-event",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        None,
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState {
            compaction_count: 1,
            last_compaction_before_tokens: Some(5000),
            last_compaction_after_tokens: Some(1500),
            last_compaction_recent_file_count: Some(0),
            last_compaction_boundary_version: Some(1),
        },
    )?;
    session_manager.save_compaction_event(
        "session-compaction-event",
        &PersistedCompactionEvent {
            event_index: 4,
            before_tokens: 12000,
            after_tokens: 3100,
            boundary_version: 3,
            replaced_start: None,
            replaced_end: None,
            metadata_owner: None,
            recent_files: vec!["src/agent/compact.rs".to_string()],
            summary: "Compacted long planning history.".to_string(),
        },
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-compaction-event")?;

    assert_eq!(snapshot.compaction.compaction_count, 4);
    assert_eq!(snapshot.compaction.before_tokens, Some(12_000));
    assert_eq!(snapshot.compaction.after_tokens, Some(3_100));
    assert_eq!(
        snapshot.compaction.summary.as_deref(),
        Some("Compacted long planning history.")
    );
    assert_eq!(
        snapshot.provenance.non_turn_rollout_source,
        ThreadNonTurnRolloutSource::StructuredEventsLog
    );
    Ok(())
}

#[test]
fn load_thread_prefers_structured_runtime_rollout_items() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    state_db.upsert_session(
        "session-runtime-rollout",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        Some("State DB summary should not override runtime rollout."),
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;
    state_db.replace_plan_steps(
        "session-runtime-rollout",
        &[PersistedPlanStep {
            step_index: 0,
            status: "pending".to_string(),
            step: "Legacy side-table plan".to_string(),
        }],
    )?;
    state_db.replace_interactions(
        "session-runtime-rollout",
        &[PersistedInteraction {
            kind: "approval".to_string(),
            status: "pending".to_string(),
            title: "Legacy Approval".to_string(),
            summary: "legacy".to_string(),
            payload: None,
        }],
    )?;
    state_db.replace_runtime_rollout_events(
        "session-runtime-rollout",
        &[
            rara_state::state_db::PersistedStructuredRolloutEvent::PlanState {
                recorded_at: None,
                explanation: Some("Structured rollout plan".to_string()),
                steps: vec![PersistedPlanStep {
                    step_index: 0,
                    status: "in_progress".to_string(),
                    step: "Structured rollout plan step".to_string(),
                }],
            },
            rara_state::state_db::PersistedStructuredRolloutEvent::Interaction {
                recorded_at: None,
                interaction: PersistedInteraction {
                    kind: "request_input".to_string(),
                    status: "completed".to_string(),
                    title: "Structured Question".to_string(),
                    summary: "answered".to_string(),
                    payload: None,
                },
            },
            rara_state::state_db::PersistedStructuredRolloutEvent::PlanLifecycle {
                recorded_at: None,
                lifecycle: PersistedPlanLifecycle {
                    phase: "plan_ready".to_string(),
                    decision: None,
                    feedback: None,
                    plan_path: Some(".rara/sessions/session-runtime-rollout/plan.md".to_string()),
                    tool_use_id: Some("exit-plan-runtime".to_string()),
                    plan_hash: None,
                },
            },
        ],
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-runtime-rollout")?;

    assert_eq!(
        snapshot.provenance.non_turn_rollout_source,
        ThreadNonTurnRolloutSource::StructuredEventsLog
    );
    assert!(matches!(
        snapshot.rollout_items.first(),
        Some(RolloutItem::PlanState { explanation, steps })
            if explanation.as_deref() == Some("Structured rollout plan")
                && steps[0].step == "Structured rollout plan step"
    ));
    assert!(matches!(
        snapshot.rollout_items.get(1),
        Some(RolloutItem::Interaction(interaction))
            if interaction.title == "Structured Question" && interaction.summary == "answered"
    ));
    assert!(matches!(
        snapshot.rollout_items.get(2),
        Some(RolloutItem::PlanLifecycle(lifecycle))
            if lifecycle.phase == "plan_ready"
                && lifecycle.tool_use_id.as_deref() == Some("exit-plan-runtime")
    ));
    Ok(())
}

#[test]
fn load_thread_falls_back_to_legacy_runtime_rollout_file() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    state_db.upsert_session(
        "session-legacy-runtime-rollout",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        Some("State DB summary should not override legacy runtime rollout."),
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;
    state_db.replace_plan_steps(
        "session-legacy-runtime-rollout",
        &[PersistedPlanStep {
            step_index: 0,
            status: "pending".to_string(),
            step: "Legacy side-table plan".to_string(),
        }],
    )?;
    state_db.replace_interactions(
        "session-legacy-runtime-rollout",
        &[PersistedInteraction {
            kind: "approval".to_string(),
            status: "pending".to_string(),
            title: "Legacy Approval".to_string(),
            summary: "legacy".to_string(),
            payload: None,
        }],
    )?;

    let runtime_path = state_db
        .rollout_root()
        .join("session-legacy-runtime-rollout")
        .join("runtime.json");
    fs::create_dir_all(runtime_path.parent().expect("runtime rollout dir"))?;
    fs::write(
        &runtime_path,
        serde_json::to_string_pretty(&vec![
            PersistedRuntimeRolloutItem::PlanState {
                explanation: Some("Legacy runtime rollout plan".to_string()),
                steps: vec![PersistedPlanStep {
                    step_index: 0,
                    status: "in_progress".to_string(),
                    step: "Inspect legacy runtime rollout".to_string(),
                }],
            },
            PersistedRuntimeRolloutItem::Interaction(PersistedInteraction {
                kind: "request_input".to_string(),
                status: "completed".to_string(),
                title: "Legacy Runtime Question".to_string(),
                summary: "answered".to_string(),
                payload: None,
            }),
        ])?,
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-legacy-runtime-rollout")?;

    assert_eq!(
        snapshot.provenance.non_turn_rollout_source,
        ThreadNonTurnRolloutSource::LegacyBackfilled
    );
    assert!(matches!(
        snapshot.rollout_items.first(),
        Some(RolloutItem::PlanState { explanation, steps })
            if explanation.as_deref() == Some("Legacy runtime rollout plan")
                && steps[0].step == "Inspect legacy runtime rollout"
    ));
    assert!(matches!(
        snapshot.rollout_items.get(1),
        Some(RolloutItem::Interaction(interaction))
            if interaction.title == "Legacy Runtime Question" && interaction.summary == "answered"
    ));
    Ok(())
}

#[test]
fn load_thread_backfills_legacy_non_turn_rollout_files_into_event_log() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    state_db.upsert_session(
        "session-legacy-non-turn",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        Some("State DB summary should not override migrated rollout."),
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;

    let rollout_dir = state_db.rollout_root().join("session-legacy-non-turn");
    fs::create_dir_all(&rollout_dir)?;
    fs::write(
        rollout_dir.join("runtime.json"),
        serde_json::to_string_pretty(&vec![
            PersistedRuntimeRolloutItem::PlanState {
                explanation: Some("Legacy runtime rollout plan".to_string()),
                steps: vec![PersistedPlanStep {
                    step_index: 0,
                    status: "in_progress".to_string(),
                    step: "Inspect migrated rollout".to_string(),
                }],
            },
            PersistedRuntimeRolloutItem::Interaction(PersistedInteraction {
                kind: "request_input".to_string(),
                status: "completed".to_string(),
                title: "Legacy Runtime Question".to_string(),
                summary: "answered".to_string(),
                payload: None,
            }),
        ])?,
    )?;

    let legacy_compactions = session_manager
        .storage_dir
        .join("session-legacy-non-turn")
        .join("compactions.json");
    fs::create_dir_all(legacy_compactions.parent().expect("legacy compaction dir"))?;
    fs::write(
        &legacy_compactions,
        serde_json::to_string_pretty(&vec![PersistedCompactionEvent {
            event_index: 1,
            before_tokens: 1200,
            after_tokens: 320,
            boundary_version: 2,
            replaced_start: None,
            replaced_end: None,
            metadata_owner: None,
            recent_files: vec!["src/thread_store.rs".to_string()],
            summary: "Legacy compaction".to_string(),
        }])?,
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-legacy-non-turn")?;

    assert_eq!(
        snapshot.provenance.non_turn_rollout_source,
        ThreadNonTurnRolloutSource::LegacyBackfilled
    );
    assert_eq!(
        snapshot.plan_explanation.as_deref(),
        Some("Legacy runtime rollout plan")
    );
    assert_eq!(snapshot.interactions.len(), 1);
    assert_eq!(snapshot.compaction.compaction_count, 1);

    let rollout_log = fs::read_to_string(rollout_dir.join("events.jsonl"))?;
    let rollout_events = rollout_log
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<PersistedStructuredRolloutEvent>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    assert!(rollout_events.iter().any(|event| matches!(
        event,
        PersistedStructuredRolloutEvent::PlanState {
            recorded_at: _,
            explanation,
            ..
        }
            if explanation.as_deref() == Some("Legacy runtime rollout plan")
    )));
    assert!(rollout_events.iter().any(|event| matches!(
        event,
        PersistedStructuredRolloutEvent::Interaction {
            recorded_at: _,
            interaction,
        }
            if interaction.title == "Legacy Runtime Question"
    )));
    assert!(rollout_events.iter().any(|event| matches!(
        event,
        PersistedStructuredRolloutEvent::Compaction {
            event_index,
            summary,
            ..
        } if *event_index == 1 && summary == "Legacy compaction"
    )));
    Ok(())
}

#[test]
fn load_thread_preserves_structured_rollout_event_order() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    state_db.upsert_session(
        "session-ordered-events",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        None,
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;
    session_manager.save_compaction_event(
        "session-ordered-events",
        &PersistedCompactionEvent {
            event_index: 1,
            before_tokens: 9000,
            after_tokens: 3200,
            boundary_version: 2,
            replaced_start: None,
            replaced_end: None,
            metadata_owner: None,
            recent_files: vec!["src/agent/compact.rs".to_string()],
            summary: "Compacted repository scan.".to_string(),
        },
    )?;
    state_db.replace_runtime_rollout_events(
        "session-ordered-events",
        &[
            rara_state::state_db::PersistedStructuredRolloutEvent::PlanState {
                recorded_at: None,
                explanation: Some("Plan after first compaction".to_string()),
                steps: vec![PersistedPlanStep {
                    step_index: 0,
                    status: "in_progress".to_string(),
                    step: "Inspect runtime events".to_string(),
                }],
            },
            rara_state::state_db::PersistedStructuredRolloutEvent::Interaction {
                recorded_at: None,
                interaction: PersistedInteraction {
                    kind: "request_input".to_string(),
                    status: "pending".to_string(),
                    title: "Question".to_string(),
                    summary: "Need confirmation".to_string(),
                    payload: None,
                },
            },
            rara_state::state_db::PersistedStructuredRolloutEvent::PlanLifecycle {
                recorded_at: None,
                lifecycle: PersistedPlanLifecycle {
                    phase: "plan_ready".to_string(),
                    decision: None,
                    feedback: None,
                    plan_path: Some(".rara/sessions/session-ordered-events/plan.md".to_string()),
                    tool_use_id: Some("exit-plan-ordered".to_string()),
                    plan_hash: None,
                },
            },
        ],
    )?;
    session_manager.save_compaction_event(
        "session-ordered-events",
        &PersistedCompactionEvent {
            event_index: 2,
            before_tokens: 6000,
            after_tokens: 2500,
            boundary_version: 3,
            replaced_start: None,
            replaced_end: None,
            metadata_owner: None,
            recent_files: vec!["src/thread_store.rs".to_string()],
            summary: "Compacted after planning.".to_string(),
        },
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-ordered-events")?;

    assert!(matches!(
        snapshot.rollout_items.first(),
        Some(RolloutItem::Compaction(compaction))
            if compaction.compaction_count == 1
    ));
    assert!(matches!(
        snapshot.rollout_items.get(1),
        Some(RolloutItem::PlanState { explanation, .. })
            if explanation.as_deref() == Some("Plan after first compaction")
    ));
    assert!(matches!(
        snapshot.rollout_items.get(2),
        Some(RolloutItem::Interaction(interaction))
            if interaction.title == "Question"
    ));
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::PlanLifecycle(lifecycle)
            if lifecycle.phase == "plan_ready"
                && lifecycle.tool_use_id.as_deref() == Some("exit-plan-ordered")
    )));
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::Compaction(compaction)
            if compaction.compaction_count == 2
    )));
    Ok(())
}

#[test]
fn load_thread_reports_state_db_fallback_for_non_turn_rollout() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    state_db.upsert_session(
        "session-state-fallback",
        "/tmp/workspace",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        Some("Fallback plan explanation."),
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;
    state_db.replace_plan_steps(
        "session-state-fallback",
        &[PersistedPlanStep {
            step_index: 0,
            status: "pending".to_string(),
            step: "Fallback plan step".to_string(),
        }],
    )?;
    state_db.replace_interactions(
        "session-state-fallback",
        &[PersistedInteraction {
            kind: "approval".to_string(),
            status: "pending".to_string(),
            title: "Fallback Approval".to_string(),
            summary: "state db only".to_string(),
            payload: None,
        }],
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread("session-state-fallback")?;

    assert_eq!(
        snapshot.provenance.non_turn_rollout_source,
        ThreadNonTurnRolloutSource::StateDbFallback
    );
    assert_eq!(
        snapshot.plan_explanation.as_deref(),
        Some("Fallback plan explanation.")
    );
    assert_eq!(snapshot.plan_steps.len(), 1);
    assert_eq!(snapshot.interactions.len(), 1);
    Ok(())
}

#[test]
fn thread_recorder_persists_runtime_state_to_metadata_and_state_db_index() -> Result<()> {
    let temp = tempdir()?;
    let state_db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    let recorder = ThreadRecorder::new(&state_db);

    recorder.persist_runtime_state(&ThreadRuntimeState {
        session_id: "session-recorder",
        cwd: "/tmp/workspace",
        branch: "main",
        provider: "ollama",
        model: "qwen3",
        base_url: Some("http://localhost:11434"),
        agent_mode: "execute",
        bash_approval: "always",
        plan_explanation: Some("Keep persistence writes structured."),
        prompt_runtime: PersistedPromptRuntimeState::default(),
        history_len: 3,
        transcript_len: 2,
        compact_state: PersistedCompactState {
            compaction_count: 1,
            last_compaction_before_tokens: Some(4000),
            last_compaction_after_tokens: Some(1200),
            last_compaction_recent_file_count: Some(2),
            last_compaction_boundary_version: Some(3),
        },
    })?;

    let threads = state_db.list_recent_thread_summaries(1)?;
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].session_id, "session-recorder");
    assert_eq!(threads[0].compaction_count, 1);
    assert_eq!(threads[0].last_compaction_after_tokens, Some(1200));
    let metadata =
        thread_metadata::load_thread_record(&state_db.rollout_root(), "session-recorder")?
            .expect("thread metadata record");
    assert_eq!(metadata.session_id, "session-recorder");
    assert_eq!(metadata.cwd, "/tmp/workspace");
    assert_eq!(metadata.model, "qwen3");
    assert_eq!(
        metadata.plan_explanation.as_deref(),
        Some("Keep persistence writes structured.")
    );
    Ok(())
}

#[test]
fn thread_recorder_appends_flushes_and_shuts_down_rollout_items() -> Result<()> {
    let temp = tempdir()?;
    let state_db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    let recorder = ThreadRecorder::new(&state_db);

    recorder.append_rollout_item(
        "session-recorder-rollout",
        &PersistedStructuredRolloutEvent::PlanState {
            recorded_at: None,
            explanation: Some("Keep rollout items append-only.".to_string()),
            steps: vec![PersistedPlanStep {
                step_index: 0,
                step: "append rollout item".to_string(),
                status: "completed".to_string(),
            }],
        },
    )?;
    recorder.flush("session-recorder-rollout")?;
    recorder.shutdown("session-recorder-rollout")?;

    let events = thread_rollout_log::load_rollout_events(
        &state_db.rollout_root(),
        "session-recorder-rollout",
    )?;
    assert!(matches!(
        events.as_slice(),
        [PersistedStructuredRolloutEvent::PlanState {
            explanation: Some(explanation),
            steps,
            ..
        }] if explanation == "Keep rollout items append-only."
            && steps.len() == 1
            && steps[0].step == "append rollout item"
    ));
    Ok(())
}

#[test]
fn thread_recorder_writes_runtime_rollout_events_directly() -> Result<()> {
    let temp = tempdir()?;
    let state_db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    let recorder = ThreadRecorder::new(&state_db);

    recorder.replace_runtime_rollout_events(
        "session-runtime-recorder",
        &[
            PersistedStructuredRolloutEvent::PlanState {
                recorded_at: None,
                explanation: Some("Runtime rollout belongs to ThreadRecorder.".to_string()),
                steps: vec![PersistedPlanStep {
                    step_index: 0,
                    step: "append canonical runtime state".to_string(),
                    status: "completed".to_string(),
                }],
            },
            PersistedStructuredRolloutEvent::Interaction {
                recorded_at: None,
                interaction: PersistedInteraction {
                    kind: "request_input".to_string(),
                    status: "completed".to_string(),
                    title: "Runtime Question".to_string(),
                    summary: "answered".to_string(),
                    payload: None,
                },
            },
            PersistedStructuredRolloutEvent::PlanLifecycle {
                recorded_at: None,
                lifecycle: PersistedPlanLifecycle {
                    phase: "plan_approved".to_string(),
                    decision: Some("approve".to_string()),
                    feedback: None,
                    plan_path: Some(".rara/sessions/session-runtime-recorder/plan.md".to_string()),
                    tool_use_id: Some("exit-plan-1".to_string()),
                    plan_hash: None,
                },
            },
        ],
    )?;

    let events = thread_rollout_log::load_rollout_events(
        &state_db.rollout_root(),
        "session-runtime-recorder",
    )?;
    assert!(matches!(
        events.as_slice(),
        [PersistedStructuredRolloutEvent::RuntimeState {
            explanation: Some(explanation),
            steps,
            interactions,
            plan_lifecycle,
            ..
        }] if explanation == "Runtime rollout belongs to ThreadRecorder."
            && steps.len() == 1
            && steps[0].step == "append canonical runtime state"
            && interactions.len() == 1
            && interactions[0].title == "Runtime Question"
            && plan_lifecycle.len() == 1
            && plan_lifecycle[0].phase == "plan_approved"
    ));
    Ok(())
}

#[test]
fn load_thread_merges_turn_log_with_state_db_fallback_turns() -> Result<()> {
    let temp = tempdir()?;
    let state_db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    let recorder = ThreadRecorder::new(&state_db);
    let store = ThreadStore::new_for_roots(
        state_db.rollout_root(),
        temp.path().join("legacy-sessions"),
        &state_db,
    );

    recorder.persist_runtime_state(&ThreadRuntimeState {
        session_id: "session-turn-log",
        cwd: "/workspace",
        branch: "main",
        provider: "openai",
        model: "gpt-5",
        base_url: None,
        agent_mode: "execute",
        bash_approval: "suggestion",
        plan_explanation: None,
        prompt_runtime: PersistedPromptRuntimeState::default(),
        history_len: 0,
        transcript_len: 0,
        compact_state: PersistedCompactState::default(),
    })?;
    recorder.persist_turn(
        "session-turn-log",
        0,
        &[PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "canonical turn log entry".to_string(),
        }],
    )?;
    state_db.persist_turn(
        "session-turn-log",
        0,
        &[PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "stale state db entry".to_string(),
        }],
    )?;
    state_db.persist_turn(
        "session-turn-log",
        1,
        &[PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "legacy state db only entry".to_string(),
        }],
    )?;

    let turn_records =
        thread_turn_log::load_turn_records(&state_db.rollout_root(), "session-turn-log")?;
    assert_eq!(turn_records.len(), 1);
    assert_eq!(
        turn_records[0].entries[0].message,
        "canonical turn log entry"
    );

    let snapshot = store.load_thread("session-turn-log")?;
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::Turn(turn) if turn.entries[0].message == "canonical turn log entry"
    )));
    assert!(!snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::Turn(turn) if turn.entries[0].message == "stale state db entry"
    )));
    assert!(snapshot.rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::Turn(turn) if turn.entries[0].message == "legacy state db only entry"
    )));
    Ok(())
}

#[test]
fn load_thread_coalesces_runtime_state_snapshots() -> Result<()> {
    let temp = tempdir()?;
    let state_db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    let recorder = ThreadRecorder::new(&state_db);
    let store = ThreadStore::new_for_roots(
        state_db.rollout_root(),
        temp.path().join("legacy-sessions"),
        &state_db,
    );

    recorder.persist_runtime_state(&ThreadRuntimeState {
        session_id: "session-runtime-snapshots",
        cwd: "/workspace",
        branch: "main",
        provider: "openai",
        model: "gpt-5",
        base_url: None,
        agent_mode: "execute",
        bash_approval: "suggestion",
        plan_explanation: None,
        prompt_runtime: PersistedPromptRuntimeState::default(),
        history_len: 0,
        transcript_len: 0,
        compact_state: PersistedCompactState::default(),
    })?;
    recorder.replace_runtime_rollout_events(
        "session-runtime-snapshots",
        &[PersistedStructuredRolloutEvent::PlanState {
            recorded_at: None,
            explanation: Some("old plan".to_string()),
            steps: vec![PersistedPlanStep {
                step_index: 0,
                status: "pending".to_string(),
                step: "old runtime state".to_string(),
            }],
        }],
    )?;
    recorder.replace_runtime_rollout_events(
        "session-runtime-snapshots",
        &[PersistedStructuredRolloutEvent::PlanState {
            recorded_at: None,
            explanation: Some("new plan".to_string()),
            steps: vec![PersistedPlanStep {
                step_index: 0,
                status: "completed".to_string(),
                step: "new runtime state".to_string(),
            }],
        }],
    )?;

    let snapshot = store.load_thread("session-runtime-snapshots")?;
    assert_eq!(snapshot.plan_explanation.as_deref(), Some("new plan"));
    assert_eq!(snapshot.plan_steps[0].step, "new runtime state");
    let plan_items = snapshot
        .rollout_items
        .iter()
        .filter_map(|item| match item {
            RolloutItem::PlanState { explanation, steps } => Some((explanation, steps)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(plan_items.len(), 1);
    assert_eq!(plan_items[0].0.as_deref(), Some("new plan"));
    assert_eq!(plan_items[0].1[0].step, "new runtime state");
    Ok(())
}

#[test]
fn thread_recorder_does_not_advance_snapshot_when_transcript_write_fails() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    let recorder = ThreadRecorder::new(&state_db);
    let session_id = "session-blocked-transcript";
    fs::create_dir_all(
        session_manager
            .storage_dir
            .join(session_id)
            .join("transcript.jsonl"),
    )?;

    let err = recorder
        .persist_history_checkpoint(
            session_id,
            &[Message {
                role: "user".to_string(),
                content: serde_json::json!("new checkpoint"),
            }],
        )
        .expect_err("transcript path should block checkpoint");

    assert!(!format!("{err:#}").is_empty());
    assert!(
        !session_manager
            .storage_dir
            .join(session_id)
            .join("history.json")
            .exists(),
        "history snapshot must not advance ahead of canonical transcript"
    );
    Ok(())
}

#[test]
fn thread_recorder_preserves_existing_lineage_on_runtime_updates() -> Result<()> {
    let temp = tempdir()?;
    let state_db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    let recorder = ThreadRecorder::new(&state_db);

    recorder.persist_runtime_state_with_lineage(
        &ThreadRuntimeState {
            session_id: "session-lineage",
            cwd: "/tmp/workspace",
            branch: "feature/fork",
            provider: "codex",
            model: "gpt-5",
            base_url: Some("https://chatgpt.com/backend-api/codex"),
            agent_mode: "execute",
            bash_approval: "suggestion",
            plan_explanation: None,
            prompt_runtime: PersistedPromptRuntimeState::default(),
            history_len: 1,
            transcript_len: 1,
            compact_state: PersistedCompactState::default(),
        },
        &ThreadRuntimeLineage {
            origin_kind: "fork".to_string(),
            forked_from_thread_id: Some("thread-parent".to_string()),
        },
    )?;

    recorder.persist_runtime_state(&ThreadRuntimeState {
        session_id: "session-lineage",
        cwd: "/tmp/workspace",
        branch: "feature/fork-updated",
        provider: "codex",
        model: "gpt-5",
        base_url: Some("https://chatgpt.com/backend-api/codex"),
        agent_mode: "plan",
        bash_approval: "always",
        plan_explanation: Some("Keep lineage stable."),
        prompt_runtime: PersistedPromptRuntimeState::default(),
        history_len: 3,
        transcript_len: 2,
        compact_state: PersistedCompactState {
            compaction_count: 1,
            ..Default::default()
        },
    })?;

    let record = state_db
        .load_thread_record("session-lineage")?
        .expect("thread record");
    assert_eq!(record.lineage.origin_kind, "fork");
    assert_eq!(
        record.lineage.forked_from_thread_id.as_deref(),
        Some("thread-parent")
    );
    assert_eq!(record.branch, "feature/fork-updated");
    assert_eq!(record.agent_mode, "plan");
    Ok(())
}

#[test]
fn list_recent_threads_exposes_thread_metadata_surface() -> Result<()> {
    let temp = tempdir()?;
    let state_db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    let recorder = ThreadRecorder::new(&state_db);

    recorder.persist_runtime_state(&ThreadRuntimeState {
        session_id: "session-thread-summary",
        cwd: "/tmp/workspace",
        branch: "feature/thread-store",
        provider: "ollama",
        model: "qwen3",
        base_url: None,
        agent_mode: "execute",
        bash_approval: "suggestion",
        plan_explanation: None,
        prompt_runtime: PersistedPromptRuntimeState::default(),
        history_len: 2,
        transcript_len: 1,
        compact_state: PersistedCompactState {
            compaction_count: 2,
            last_compaction_before_tokens: Some(4096),
            last_compaction_after_tokens: Some(1536),
            last_compaction_recent_file_count: Some(1),
            last_compaction_boundary_version: Some(4),
        },
    })?;
    state_db.persist_turn(
        "session-thread-summary",
        0,
        &[PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "Preview line".to_string(),
        }],
    )?;

    let threads = ThreadStore::list_recent_threads_for_db(&state_db, 5)?;

    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].metadata.session_id, "session-thread-summary");
    assert_eq!(threads[0].metadata.branch, "feature/thread-store");
    assert_eq!(threads[0].metadata.cwd, "/tmp/workspace");
    assert_eq!(threads[0].metadata.agent_mode, "execute");
    assert_eq!(threads[0].metadata.bash_approval, "suggestion");
    assert_eq!(threads[0].metadata.history_len, 2);
    assert_eq!(threads[0].metadata.transcript_len, 1);
    assert_eq!(threads[0].preview, "Agent: Preview line");
    assert_eq!(threads[0].compaction.compaction_count, 2);
    assert_eq!(threads[0].compaction.after_tokens, Some(1536));
    Ok(())
}

#[test]
fn latest_thread_summary_uses_thread_summary_contract() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;

    state_db.upsert_session_with_lineage(
        "thread-old",
        "/tmp/workspace-old",
        "main",
        "ollama",
        "qwen3",
        None,
        "execute",
        "always",
        &rara_state::state_db::PersistedThreadLineage::default(),
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        1,
        &PersistedCompactState::default(),
    )?;
    state_db.persist_turn(
        "thread-old",
        0,
        &[PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "Older preview".to_string(),
        }],
    )?;
    std::thread::sleep(Duration::from_secs(1));
    state_db.upsert_session_with_lineage(
        "thread-new",
        "/tmp/workspace-new",
        "feature",
        "codex",
        "gpt-5",
        Some("https://chatgpt.com/backend-api/codex"),
        "plan",
        "on-request",
        &rara_state::state_db::PersistedThreadLineage {
            origin_kind: "fork".to_string(),
            forked_from_thread_id: Some("thread-old".to_string()),
        },
        None,
        &PersistedPromptRuntimeState::default(),
        2,
        1,
        &PersistedCompactState {
            compaction_count: 2,
            ..Default::default()
        },
    )?;
    state_db.persist_turn(
        "thread-new",
        0,
        &[PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "Newer preview".to_string(),
        }],
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let latest = store
        .latest_thread_summary()?
        .expect("latest thread should exist");

    assert_eq!(latest.metadata.session_id, "thread-new");
    assert_eq!(latest.metadata.origin_kind, "fork");
    assert_eq!(
        latest.metadata.forked_from_thread_id.as_deref(),
        Some("thread-old")
    );
    assert_eq!(latest.preview, "Agent: Newer preview");
    assert_eq!(latest.compaction.compaction_count, 2);
    Ok(())
}

#[test]
fn export_thread_markdown_renders_metadata_summary_and_messages() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    session_manager.save_session(
        "thread-export",
        &[
            Message {
                role: "user".to_string(),
                content: serde_json::json!([{"type": "text", "text": "How should\nmemory work?"}]),
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::json!("Keep durable memory separate from raw threads."),
            },
        ],
    )?;
    state_db.upsert_session(
        "thread-export",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "execute",
        "always",
        None,
        &PersistedPromptRuntimeState::default(),
        2,
        2,
        &PersistedCompactState::default(),
    )?;
    session_manager.save_compaction_event(
        "thread-export",
        &PersistedCompactionEvent {
            event_index: 1,
            before_tokens: 3000,
            after_tokens: 900,
            boundary_version: 1,
            replaced_start: None,
            replaced_end: None,
            metadata_owner: None,
            recent_files: vec!["src/memory_store.rs".to_string()],
            summary: "Memory records are durable; threads are source material.".to_string(),
        },
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let markdown = store.export_thread_markdown("thread-export")?;

    assert!(markdown.contains("session_id: \"thread-export\""));
    assert!(markdown.contains("# How should memory work?"));
    assert!(markdown.contains("## Summary"));
    assert!(markdown.contains("Memory records are durable"));
    assert!(markdown.contains("## User"));
    assert!(markdown.contains("How should\nmemory work?"));
    assert!(markdown.contains("## Assistant"));
    assert!(markdown.contains("Keep durable memory separate"));
    Ok(())
}

#[tokio::test]
async fn distill_thread_summary_persists_thread_linked_memory_record() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir.clone())?;
    session_manager.save_session(
        "thread-distill",
        &[Message {
            role: "user".to_string(),
            content: serde_json::json!("Continue memory backend implementation"),
        }],
    )?;
    state_db.upsert_session(
        "thread-distill",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "execute",
        "always",
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        1,
        &PersistedCompactState::default(),
    )?;
    session_manager.save_compaction_event(
        "thread-distill",
        &PersistedCompactionEvent {
            event_index: 1,
            before_tokens: 4000,
            after_tokens: 1200,
            boundary_version: 1,
            replaced_start: None,
            replaced_end: None,
            metadata_owner: None,
            recent_files: vec!["src/thread_store.rs".to_string()],
            summary: "Thread lifecycle APIs should export markdown and distill durable memory."
                .to_string(),
        },
    )?;
    let memory_store = MemoryStore::new(
        std::sync::Arc::new(MockLlm),
        std::sync::Arc::new(VectorDB::new(
            rara_dir.join("lancedb").to_str().expect("utf8 path"),
        )),
    );
    let store = ThreadStore::new(&session_manager, &state_db);

    let memory = store
        .distill_thread_summary(&memory_store, "thread-distill")
        .await?
        .expect("distilled memory");

    assert_eq!(memory.source, MemorySource::ThreadDistill);
    assert_eq!(memory.scope, MemoryScope::Thread);
    assert_eq!(memory.session_id.as_deref(), Some("thread-distill"));
    assert_eq!(memory.thread_id.as_deref(), Some("thread-distill"));
    assert!(
        memory
            .content
            .contains("Thread lifecycle APIs should export markdown")
    );
    let reloaded = memory_store
        .get(&memory.id)
        .await?
        .expect("persisted memory record");
    assert_eq!(reloaded.thread_id.as_deref(), Some("thread-distill"));
    Ok(())
}

#[tokio::test]
async fn distill_thread_memories_persists_multiple_deduped_records() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir.clone())?;
    session_manager.save_session(
        "thread-distill-many",
        &[
            Message {
                role: "user".to_string(),
                content: serde_json::json!("How should memory promotion work?"),
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::json!(
                    "Use session shards first, then distill durable takeaways into MemoryRecords."
                ),
            },
        ],
    )?;
    state_db.upsert_session(
        "thread-distill-many",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "execute",
        "always",
        None,
        &PersistedPromptRuntimeState::default(),
        2,
        2,
        &PersistedCompactState::default(),
    )?;
    let memory_store = MemoryStore::new(
        Arc::new(DistillMockLlm),
        Arc::new(VectorDB::new(
            rara_dir.join("lancedb").to_str().expect("utf8 path"),
        )),
    );
    let store = ThreadStore::new(&session_manager, &state_db);

    let memories = store
        .distill_thread_memories(&memory_store, "thread-distill-many")
        .await?;

    assert_eq!(memories.len(), 2);
    assert_eq!(memories[0].source, MemorySource::ThreadDistill);
    assert_eq!(memories[0].scope, MemoryScope::Thread);
    assert_eq!(
        memories[0].session_id.as_deref(),
        Some("thread-distill-many")
    );
    assert_eq!(memories[0].labels, vec![MemoryLabel::Decision]);
    assert!(
        memories
            .iter()
            .any(|memory| memory.title == "Session shards before global memory")
    );
    assert!(
        memories
            .iter()
            .any(|memory| memory.title == "Promotion records keep provenance")
    );
    let labels = memory_store.list_labels(Some(MemoryScope::Thread)).await?;
    assert!(
        labels
            .iter()
            .any(|label| label.label == MemoryLabel::Decision)
    );
    Ok(())
}

struct DistillMockLlm;

#[async_trait]
impl LlmBackend for DistillMockLlm {
    async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: serde_json::json!({
                    "memories": [
                        {
                            "title": "Session shards before global memory",
                            "content": "Use per-session append shards for raw checkpoints before promoting durable takeaways into global MemoryRecords.",
                            "labels": ["decision"],
                            "importance": 0.8
                        },
                        {
                            "title": "Session shards before global memory",
                            "content": "Use per-session append shards for raw checkpoints before promoting durable takeaways into global MemoryRecords.",
                            "labels": ["decision"],
                            "importance": 0.8
                        },
                        {
                            "title": "Promotion records keep provenance",
                            "content": "Thread distillation should preserve session_id, thread_id, and source span on generated MemoryRecords.",
                            "labels": ["procedure"],
                            "importance": 0.7
                        }
                    ]
                })
                .to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1; 128])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".to_string())
    }
}

#[test]
fn fork_thread_preserves_materialized_state_and_sets_lineage() -> Result<()> {
    let temp = tempdir()?;
    let rara_dir = temp.path().join(".rara");
    let session_manager = SessionManager::new_for_rara_dir(rara_dir.clone())?;
    let state_db = StateDb::new_for_root_dir(rara_dir)?;
    session_manager.save_session(
        "source-thread",
        &[Message {
            role: "user".to_string(),
            content: serde_json::json!("continue this implementation"),
        }],
    )?;
    state_db.upsert_session(
        "source-thread",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        Some("https://chatgpt.com/backend-api/codex"),
        "execute",
        "on-request",
        Some("Preserve thread continuity."),
        &PersistedPromptRuntimeState {
            append_system_prompt: Some("Keep the fork aligned with the parent thread.".to_string()),
            warnings: vec!["using local thread store".to_string()],
        },
        1,
        1,
        &PersistedCompactState {
            compaction_count: 2,
            last_compaction_before_tokens: Some(9000),
            last_compaction_after_tokens: Some(2400),
            last_compaction_recent_file_count: Some(1),
            last_compaction_boundary_version: Some(3),
        },
    )?;
    state_db.replace_plan_steps(
        "source-thread",
        &[PersistedPlanStep {
            step_index: 0,
            status: "in_progress".to_string(),
            step: "Implement fork lifecycle".to_string(),
        }],
    )?;
    state_db.replace_interactions(
        "source-thread",
        &[PersistedInteraction {
            kind: "approval".to_string(),
            status: "completed".to_string(),
            title: "Approved".to_string(),
            summary: "continue".to_string(),
            payload: None,
        }],
    )?;
    state_db.persist_turn(
        "source-thread",
        0,
        &[PersistedTurnEntry {
            role: "Agent".to_string(),
            message: "Implementing the fork command.".to_string(),
        }],
    )?;
    session_manager.save_compaction_event(
        "source-thread",
        &PersistedCompactionEvent {
            event_index: 2,
            before_tokens: 9000,
            after_tokens: 2400,
            boundary_version: 3,
            replaced_start: Some(0),
            replaced_end: Some(3),
            metadata_owner: Some("runtime.compaction".to_string()),
            recent_files: vec!["src/thread_store.rs".to_string()],
            summary: "Compacted earlier lifecycle exploration.".to_string(),
        },
    )?;

    let store = ThreadStore::new(&session_manager, &state_db);
    let forked_thread_id = store.fork_thread("source-thread")?;
    assert_ne!(forked_thread_id, "source-thread");

    let snapshot = store.load_thread(&forked_thread_id)?;
    assert_eq!(snapshot.metadata.origin_kind, "fork");
    assert_eq!(
        snapshot.metadata.forked_from_thread_id.as_deref(),
        Some("source-thread")
    );
    assert_eq!(snapshot.history.len(), 1);
    assert_eq!(
        snapshot.plan_explanation.as_deref(),
        Some("Preserve thread continuity.")
    );
    assert_eq!(snapshot.plan_steps.len(), 1);
    assert_eq!(snapshot.interactions.len(), 1);
    assert_eq!(snapshot.compaction.compaction_count, 2);
    assert!(matches!(
        snapshot.rollout_items.last(),
        Some(RolloutItem::Turn(turn))
            if turn.entries[0].message == "Implementing the fork command."
    ));

    let runtime_state = state_db
        .load_session_runtime_state(&forked_thread_id)?
        .expect("forked runtime state");
    assert_eq!(
        runtime_state.prompt_runtime.append_system_prompt.as_deref(),
        Some("Keep the fork aligned with the parent thread.")
    );
    assert_eq!(
        runtime_state.prompt_runtime.warnings,
        vec!["using local thread store".to_string()]
    );

    let rollout_events = state_db.load_rollout_events(&forked_thread_id)?;
    assert!(rollout_events.iter().any(|event| matches!(
        event,
        PersistedStructuredRolloutEvent::Compaction {
            event_index,
            replaced_start,
            replaced_end,
            metadata_owner,
            summary,
            ..
        } if *event_index == 2
            && *replaced_start == Some(0)
            && *replaced_end == Some(3)
            && metadata_owner.as_deref() == Some("runtime.compaction")
            && summary == "Compacted earlier lifecycle exploration."
    )));
    assert!(rollout_events.iter().any(|event| matches!(
        event,
        PersistedStructuredRolloutEvent::RuntimeState {
            recorded_at: _,
            explanation,
            steps,
            interactions,
            plan_lifecycle,
        }
            if explanation.as_deref() == Some("Preserve thread continuity.")
                && steps.len() == 1
                && interactions.len() == 1
                && plan_lifecycle.is_empty()
    )));

    Ok(())
}
