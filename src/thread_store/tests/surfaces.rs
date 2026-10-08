use super::*;

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
        std::sync::Arc::new(MemoryHandle::new(
            rara_dir.join("memory").to_str().expect("utf8 path"),
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
        Arc::new(MemoryHandle::new(
            rara_dir.join("memory").to_str().expect("utf8 path"),
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
    state_db.replace_runtime_rollout_events(
        "source-thread",
        &[PersistedStructuredRolloutEvent::RuntimeState {
            recorded_at: None,
            explanation: Some("Preserve thread continuity.".to_string()),
            steps: vec![PersistedPlanStep {
                step_index: 0,
                status: "in_progress".to_string(),
                step: "Implement fork lifecycle".to_string(),
            }],
            interactions: vec![PersistedInteraction {
                kind: "approval".to_string(),
                status: "completed".to_string(),
                title: "Approved".to_string(),
                summary: "continue".to_string(),
                payload: None,
            }],
            plan_lifecycle: vec![PersistedPlanLifecycle {
                phase: "plan_ready".to_string(),
                decision: None,
                feedback: None,
                plan_path: Some(".rara/sessions/source-thread/plan.md".to_string()),
                tool_use_id: Some("exit-plan-source".to_string()),
                plan_hash: None,
                submitted_at: None,
                decided_at: None,
            }],
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
                && plan_lifecycle.len() == 1
                && plan_lifecycle[0].phase == "plan_ready"
                && plan_lifecycle[0].tool_use_id.as_deref() == Some("exit-plan-source")
    )));

    Ok(())
}
