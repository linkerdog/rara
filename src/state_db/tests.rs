use anyhow::Result;
use rusqlite::Connection;
use tempfile::tempdir;

use super::{
    PersistedCompactState, PersistedInteraction, PersistedPlanStep, PersistedPromptRuntimeState,
    PersistedStructuredRolloutEvent, PersistedTurnEntry, StateDb,
};

#[test]
fn persists_metadata_and_rollout_artifact() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    db.upsert_session(
        "session-1",
        "/tmp/workspace",
        "main",
        "ollama",
        "gemma4:e4b",
        Some("http://localhost:11434"),
        "execute",
        "suggestion",
        Some("Inspect the repository and summarize issues."),
        &PersistedPromptRuntimeState::default(),
        4,
        3,
        &PersistedCompactState::default(),
    )?;
    db.replace_plan_steps(
        "session-1",
        &[PersistedPlanStep {
            step_index: 0,
            status: "in_progress".to_string(),
            step: "Inspect src/main.rs".to_string(),
        }],
    )?;
    db.replace_interactions(
        "session-1",
        &[PersistedInteraction {
            kind: "approval".to_string(),
            status: "completed".to_string(),
            title: "Approval Completed".to_string(),
            summary: "run once".to_string(),
            payload: None,
        }],
    )?;
    let summary = db.persist_turn(
        "session-1",
        0,
        &[
            PersistedTurnEntry {
                role: "You".to_string(),
                message: "hello".to_string(),
            },
            PersistedTurnEntry {
                role: "Agent".to_string(),
                message: "world".to_string(),
            },
        ],
    )?;

    let verify = Connection::open(db.path())?;
    let session_count: i64 = verify.query_row(
        "SELECT COUNT(*) FROM sessions WHERE id = 'session-1'",
        [],
        |row| row.get(0),
    )?;
    let turn_count: i64 = verify.query_row(
        "SELECT COUNT(*) FROM turns WHERE session_id = 'session-1'",
        [],
        |row| row.get(0),
    )?;
    let plan_count: i64 = verify.query_row(
        "SELECT COUNT(*) FROM plan_steps WHERE session_id = 'session-1'",
        [],
        |row| row.get(0),
    )?;
    let interaction_count: i64 = verify.query_row(
        "SELECT COUNT(*) FROM interactions WHERE session_id = 'session-1'",
        [],
        |row| row.get(0),
    )?;

    assert_eq!(session_count, 1);
    assert_eq!(turn_count, 1);
    assert_eq!(plan_count, 1);
    assert_eq!(interaction_count, 1);
    assert_eq!(summary.event_count, 2);

    let artifact = db.rollout_root().join("session-1").join("000000.json");
    assert!(artifact.exists());
    let loaded = db.load_turn_entries("session-1", 0)?;
    assert_eq!(loaded.len(), 2);
    Ok(())
}

#[test]
fn persists_interaction_payloads_for_restore() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    db.upsert_session(
        "session-2",
        "/tmp/workspace",
        "main",
        "ollama",
        "gemma4",
        Some("http://localhost:11434"),
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        2,
        1,
        &PersistedCompactState::default(),
    )?;
    db.replace_interactions(
        "session-2",
        &[PersistedInteraction {
            kind: "approval".to_string(),
            status: "pending".to_string(),
            title: "Pending Approval".to_string(),
            summary: "cargo test".to_string(),
            payload: Some(serde_json::json!({
                "tool_use_id": "tool-42",
                "command": "cargo test",
                "allow_net": true
            })),
        }],
    )?;

    let interactions = db.load_interactions("session-2")?;
    assert_eq!(interactions.len(), 1);
    let payload = interactions[0].payload.as_ref().expect("payload");
    assert_eq!(
        payload.get("tool_use_id").and_then(|v| v.as_str()),
        Some("tool-42")
    );
    assert_eq!(
        payload.get("command").and_then(|v| v.as_str()),
        Some("cargo test")
    );
    assert_eq!(
        payload.get("allow_net").and_then(|v| v.as_bool()),
        Some(true)
    );
    Ok(())
}

#[test]
fn persists_structured_approval_payloads_for_restore() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    db.upsert_session(
        "session-structured",
        "/tmp/workspace",
        "main",
        "ollama",
        "gemma4",
        Some("http://localhost:11434"),
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        2,
        1,
        &PersistedCompactState::default(),
    )?;
    db.replace_interactions(
        "session-structured",
        &[PersistedInteraction {
            kind: "approval".to_string(),
            status: "pending".to_string(),
            title: "Pending Approval".to_string(),
            summary: "cargo check --workspace".to_string(),
            payload: Some(serde_json::json!({
                "tool_use_id": "tool-99",
                "program": "cargo",
                "args": ["check", "--workspace"],
                "cwd": "/tmp/workspace",
                "env": { "RUST_LOG": "debug" },
                "allow_net": false
            })),
        }],
    )?;

    let interactions = db.load_interactions("session-structured")?;
    assert_eq!(interactions.len(), 1);
    let payload = interactions[0].payload.as_ref().expect("payload");
    assert_eq!(
        payload.get("tool_use_id").and_then(|v| v.as_str()),
        Some("tool-99")
    );
    assert_eq!(
        payload.get("program").and_then(|v| v.as_str()),
        Some("cargo")
    );
    assert_eq!(
        payload
            .get("args")
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
            .and_then(|v| v.as_str()),
        Some("check")
    );
    assert_eq!(
        payload.get("cwd").and_then(|v| v.as_str()),
        Some("/tmp/workspace")
    );
    assert_eq!(
        payload
            .get("env")
            .and_then(|v| v.get("RUST_LOG"))
            .and_then(|v| v.as_str()),
        Some("debug")
    );
    assert_eq!(
        payload.get("allow_net").and_then(|v| v.as_bool()),
        Some(false)
    );
    Ok(())
}

#[test]
fn persists_compact_state_for_restore() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    db.upsert_session(
        "session-compact",
        "/tmp/workspace",
        "main",
        "ollama",
        "gemma4",
        Some("http://localhost:11434"),
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        6,
        2,
        &PersistedCompactState {
            compaction_count: 3,
            last_compaction_before_tokens: Some(12_000),
            last_compaction_after_tokens: Some(4_200),
            last_compaction_recent_file_count: Some(2),
            last_compaction_boundary_version: Some(1),
        },
    )?;

    let compact_state = db.load_session_compact_state("session-compact")?;
    assert_eq!(compact_state.compaction_count, 3);
    assert_eq!(compact_state.last_compaction_before_tokens, Some(12_000));
    assert_eq!(compact_state.last_compaction_after_tokens, Some(4_200));
    assert_eq!(compact_state.last_compaction_recent_file_count, Some(2));
    assert_eq!(compact_state.last_compaction_boundary_version, Some(1));

    let threads = db.list_recent_thread_summaries(5)?;
    let summary = threads
        .iter()
        .find(|item| item.session_id == "session-compact")
        .expect("recent thread summary");
    assert_eq!(summary.compaction_count, 3);
    assert_eq!(summary.last_compaction_before_tokens, Some(12_000));
    assert_eq!(summary.last_compaction_after_tokens, Some(4_200));
    assert_eq!(summary.last_compaction_recent_file_count, Some(2));
    assert_eq!(summary.last_compaction_boundary_version, Some(1));

    let recent_threads = db.list_recent_thread_records(5)?;
    let recent = recent_threads
        .iter()
        .find(|item| item.session_id == "session-compact")
        .expect("recent thread record");
    assert_eq!(recent.cwd, "/tmp/workspace");
    assert_eq!(recent.provider, "ollama");
    assert_eq!(recent.model, "gemma4");
    assert_eq!(recent.agent_mode, "execute");
    assert_eq!(recent.bash_approval, "suggestion");
    assert_eq!(recent.compaction_count, 3);
    assert_eq!(recent.last_compaction_after_tokens, Some(4_200));
    Ok(())
}

#[test]
fn recent_threads_ignore_sessions_without_resume_data() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;

    db.upsert_session(
        "slash-resume-only",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        0,
        3,
        &PersistedCompactState::default(),
    )?;
    assert_eq!(db.latest_thread_id()?, None);

    db.upsert_session(
        "history-backed",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        0,
        &PersistedCompactState::default(),
    )?;
    db.upsert_session(
        "plan-backed",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "plan",
        "suggestion",
        Some("Plan-only sessions should remain resumable."),
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;
    db.upsert_session(
        "compact-backed",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState {
            compaction_count: 1,
            ..PersistedCompactState::default()
        },
    )?;
    db.upsert_session(
        "turn-backed",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;
    db.persist_turn(
        "turn-backed",
        0,
        &[PersistedTurnEntry {
            role: "User".to_string(),
            message: "Real request".to_string(),
        }],
    )?;

    let summaries = db.list_recent_thread_summaries(10)?;
    let summary_ids = summaries
        .iter()
        .map(|thread| thread.session_id.as_str())
        .collect::<Vec<_>>();
    assert!(!summary_ids.contains(&"slash-resume-only"));
    assert!(summary_ids.contains(&"history-backed"));
    assert!(summary_ids.contains(&"plan-backed"));
    assert!(summary_ids.contains(&"compact-backed"));
    assert!(summary_ids.contains(&"turn-backed"));

    let records = db.list_recent_thread_records(10)?;
    let record_ids = records
        .iter()
        .map(|thread| thread.session_id.as_str())
        .collect::<Vec<_>>();
    assert!(!record_ids.contains(&"slash-resume-only"));
    assert!(record_ids.contains(&"history-backed"));
    assert!(record_ids.contains(&"plan-backed"));
    assert!(record_ids.contains(&"compact-backed"));
    assert!(record_ids.contains(&"turn-backed"));
    Ok(())
}

#[test]
fn latest_thread_id_keeps_plan_and_compaction_only_sessions_resumable() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;

    db.upsert_session(
        "plan-only",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "plan",
        "suggestion",
        Some("Plan approval is enough resume state."),
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState::default(),
    )?;
    assert_eq!(db.latest_thread_id()?.as_deref(), Some("plan-only"));

    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    db.upsert_session(
        "compact-only",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        0,
        0,
        &PersistedCompactState {
            compaction_count: 1,
            ..PersistedCompactState::default()
        },
    )?;
    assert_eq!(db.latest_thread_id()?.as_deref(), Some("compact-only"));
    Ok(())
}

#[test]
fn load_rollout_events_prefers_append_only_log_without_snapshot_rewrite() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;

    db.append_compaction_rollout_event(
        "session-events",
        2,
        8000,
        2400,
        3,
        &["src/thread_store.rs".to_string()],
        "Compacted history",
    )?;
    db.replace_runtime_rollout_events(
        "session-events",
        &[PersistedStructuredRolloutEvent::PlanState {
            recorded_at: None,
            explanation: Some("Structured runtime plan".to_string()),
            steps: vec![PersistedPlanStep {
                step_index: 0,
                status: "pending".to_string(),
                step: "Inspect thread store".to_string(),
            }],
        }],
    )?;

    assert!(
        !db.rollout_root()
            .join("session-events")
            .join("events.json")
            .exists()
    );
    assert!(
        db.rollout_root()
            .join("session-events")
            .join("events.jsonl")
            .exists()
    );

    let events = db.load_rollout_events("session-events")?;
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        PersistedStructuredRolloutEvent::Compaction {
            event_index,
            summary,
            ..
        } if *event_index == 2 && summary == "Compacted history"
    ));
    assert!(matches!(
        &events[1],
        PersistedStructuredRolloutEvent::RuntimeState {
            recorded_at: _,
            explanation,
            steps,
            interactions,
        }
            if explanation.as_deref() == Some("Structured runtime plan")
                && steps.len() == 1
                && steps[0].step == "Inspect thread store"
                && interactions.is_empty()
    ));
    Ok(())
}

#[test]
fn indexes_spawn_agent_edges_for_listing_queries() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    db.sync_spawn_agent_edges_from_events(
        "parent-session",
        &[
            PersistedStructuredRolloutEvent::PlanState {
                recorded_at: Some(9),
                explanation: Some("ignored".to_string()),
                steps: Vec::new(),
            },
            PersistedStructuredRolloutEvent::SpawnAgent {
                recorded_at: Some(10),
                event_id: "spawn-1".to_string(),
                agent_id: "worker-1".to_string(),
                name: Some("Worker 1".to_string()),
                child_session_id: "child-1".to_string(),
                status: "done".to_string(),
                summary: Some("Finished.".to_string()),
            },
        ],
    )?;

    let edges = db.load_spawn_agent_edges("parent-session")?;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].parent_session_id, "parent-session");
    assert_eq!(edges[0].event_id, "spawn-1");
    assert_eq!(edges[0].agent_id, "worker-1");
    assert_eq!(edges[0].name.as_deref(), Some("Worker 1"));
    assert_eq!(edges[0].child_session_id, "child-1");
    assert_eq!(edges[0].status, "done");
    assert_eq!(edges[0].summary.as_deref(), Some("Finished."));
    assert_eq!(edges[0].recorded_at, Some(10));

    let original_id: i64 = {
        let verify = Connection::open(db.path())?;
        verify.query_row(
            "SELECT id FROM spawn_agent_edges WHERE parent_session_id = 'parent-session'",
            [],
            |row| row.get(0),
        )?
    };
    db.sync_spawn_agent_edges_from_events(
        "parent-session",
        &[PersistedStructuredRolloutEvent::SpawnAgent {
            recorded_at: Some(10),
            event_id: "spawn-1".to_string(),
            agent_id: "worker-1".to_string(),
            name: Some("Worker 1".to_string()),
            child_session_id: "child-1".to_string(),
            status: "done".to_string(),
            summary: Some("Finished.".to_string()),
        }],
    )?;
    let id_after_noop_sync: i64 = {
        let verify = Connection::open(db.path())?;
        verify.query_row(
            "SELECT id FROM spawn_agent_edges WHERE parent_session_id = 'parent-session'",
            [],
            |row| row.get(0),
        )?
    };
    assert_eq!(id_after_noop_sync, original_id);
    Ok(())
}

#[test]
fn load_legacy_rollout_migration_collects_structured_and_runtime_fallbacks() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    let rollout_dir = db.rollout_root().join("session-migration");
    std::fs::create_dir_all(&rollout_dir)?;
    std::fs::write(
        rollout_dir.join("events.json"),
        serde_json::to_string_pretty(&vec![PersistedStructuredRolloutEvent::PlanState {
            recorded_at: None,
            explanation: Some("Legacy structured plan".to_string()),
            steps: vec![PersistedPlanStep {
                step_index: 0,
                status: "pending".to_string(),
                step: "Read old events snapshot".to_string(),
            }],
        }])?,
    )?;
    std::fs::write(
        rollout_dir.join("runtime.json"),
        serde_json::to_string_pretty(&vec![super::PersistedRuntimeRolloutItem::Interaction(
            PersistedInteraction {
                kind: "request_input".to_string(),
                status: "completed".to_string(),
                title: "Legacy runtime interaction".to_string(),
                summary: "answered".to_string(),
                payload: None,
            },
        )])?,
    )?;

    let migration = db.load_legacy_rollout_migration("session-migration")?;

    assert_eq!(migration.structured_events.len(), 1);
    assert_eq!(migration.runtime_rollout.len(), 1);
    assert!(matches!(
        &migration.structured_events[0],
        PersistedStructuredRolloutEvent::PlanState {
            recorded_at: _,
            explanation,
            steps,
        }
            if explanation.as_deref() == Some("Legacy structured plan")
                && steps[0].step == "Read old events snapshot"
    ));
    assert!(matches!(
        &migration.runtime_rollout[0],
        super::PersistedRuntimeRolloutItem::Interaction(interaction)
            if interaction.title == "Legacy runtime interaction"
    ));
    Ok(())
}

#[test]
fn persists_session_runtime_state_for_restore() -> Result<()> {
    let temp = tempdir()?;
    let db = StateDb::new_for_root_dir(temp.path().join(".rara"))?;
    db.upsert_session(
        "session-runtime",
        "/tmp/workspace",
        "main",
        "codex",
        "gpt-5.4",
        Some("https://chatgpt.com/backend-api/codex"),
        "plan",
        "always",
        Some("Restore should rebuild the same context surface."),
        &PersistedPromptRuntimeState {
            append_system_prompt: Some("appendix".to_string()),
            warnings: vec!["missing custom prompt file".to_string()],
        },
        8,
        3,
        &PersistedCompactState::default(),
    )?;

    let runtime = db
        .load_session_runtime_state("session-runtime")?
        .expect("session runtime state");
    assert_eq!(runtime.agent_mode, "plan");
    assert_eq!(runtime.bash_approval, "always");
    assert_eq!(
        runtime.prompt_runtime.append_system_prompt.as_deref(),
        Some("appendix")
    );
    assert_eq!(
        runtime.prompt_runtime.warnings,
        vec!["missing custom prompt file".to_string()]
    );
    Ok(())
}
