use super::*;

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
                    submitted_at: None,
                    decided_at: None,
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
    assert!(matches!(
        snapshot.rollout_items.get(3),
        Some(RolloutItem::PlanLifecycle(lifecycle))
            if lifecycle.phase == "plan_ready"
                && lifecycle.tool_use_id.as_deref() == Some("exit-plan-ordered")
    ));
    assert!(matches!(
        snapshot.rollout_items.get(4),
        Some(RolloutItem::Compaction(compaction))
            if compaction.compaction_count == 2
    ));
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
                    submitted_at: None,
                    decided_at: None,
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
