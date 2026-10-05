use super::*;

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
            title: None,
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
                    submitted_at: None,
                    decided_at: None,
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
