    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;
    use crate::agent::Message;
    use crate::llm::{
        ContentBlock, EmbeddingBackend, EmbeddingInputKind, LlmBackend, LlmResponse, MockLlm,
    };

    struct FailingLlmBackend;

    #[async_trait]
    impl LlmBackend for FailingLlmBackend {
        async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "ok".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            })
        }

        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Err(anyhow!("llm embedding path should stay unused"))
        }

        async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
            Ok("summary".to_string())
        }
    }

    struct FixedEmbeddingBackend;

    #[async_trait]
    impl EmbeddingBackend for FixedEmbeddingBackend {
        async fn embed(&self, text: &str, kind: EmbeddingInputKind) -> Result<Vec<f32>> {
            let vector = match kind {
                EmbeddingInputKind::Query if text.contains("parser") => vec![1.0, 0.0, 0.0, 0.0],
                EmbeddingInputKind::Document if text.contains("DeepSeek") => {
                    vec![1.0, 0.0, 0.0, 0.0]
                }
                _ => vec![0.0, 1.0, 0.0, 0.0],
            };
            Ok(vector)
        }
    }

    fn test_memory_record(id: &str, content: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            title: title_from_content(content),
            content: content.to_string(),
            labels: vec![MemoryLabel::Fact],
            importance: DEFAULT_IMPORTANCE,
            pinned: false,
            source: MemorySource::UserCreated,
            scope: MemoryScope::Project,
            session_id: None,
            thread_id: None,
            source_span: None,
            created_at_unix_seconds: 1,
            updated_at_unix_seconds: 1,
        }
    }

    #[test]
    fn memory_record_file_store_writes_compact_json_and_reads_legacy_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("records.json");
        let legacy_path = temp.path().join("legacy_records.json");
        let record = test_memory_record("memory-test-1", "Compact storage should stay readable.");
        let file = PersistedMemoryRecordFile {
            version: MEMORY_RECORDS_FILE_VERSION,
            records: vec![record.clone()],
        };

        write_record_file_sync(&path, &file).expect("write compact record file");
        let content = fs::read_to_string(&path).expect("read compact record file");
        assert!(!content.contains('\n'));
        assert_eq!(
            load_records_sync(&path).expect("load compact records"),
            vec![record.clone()]
        );

        fs::write(
            &legacy_path,
            serde_json::to_string(&vec![record.clone()]).expect("serialize legacy records"),
        )
        .expect("write legacy record file");
        assert_eq!(
            load_records_sync(&legacy_path).expect("load legacy records"),
            vec![record]
        );
    }

    #[test]
    fn memory_record_index_scope_key_respects_memory_scope() {
        let mut workspace = test_memory_record(
            "memory-workspace",
            "Workspace-scoped records may carry thread provenance.",
        );
        workspace.scope = MemoryScope::Workspace;
        workspace.session_id = Some("session-1".to_string());
        workspace.thread_id = Some("thread-1".to_string());
        assert_eq!(workspace.index_scope_key(), "workspace");

        let mut thread = workspace.clone();
        thread.scope = MemoryScope::Thread;
        assert_eq!(thread.index_scope_key(), "thread-1");

        let mut session = workspace.clone();
        session.scope = MemoryScope::Session;
        assert_eq!(session.index_scope_key(), "session-1");

        let mut project = workspace;
        project.scope = MemoryScope::Project;
        assert_eq!(project.index_scope_key(), "project");
    }

    #[test]
    fn memory_promotion_base_enforces_scope_rules() {
        let workspace = NewMemoryRecord::promotion_base(
            MemorySource::ProtocolWrite,
            MemoryPromotionTarget::Workspace {
                session_id: Some(" session-1 ".to_string()),
                thread_id: Some(" thread-1 ".to_string()),
            },
        )
        .expect("workspace promotion");
        assert_eq!(workspace.scope, MemoryScope::Workspace);
        assert_eq!(workspace.session_id.as_deref(), Some("session-1"));
        assert_eq!(workspace.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(workspace.source_span, None);

        let thread_err = NewMemoryRecord::promotion_base(
            MemorySource::ThreadDistill,
            MemoryPromotionTarget::Thread {
                session_id: Some("session-1".to_string()),
                thread_id: " ".to_string(),
                source_span: None,
            },
        )
        .expect_err("thread promotion needs a thread id");
        assert!(thread_err.to_string().contains("thread_id is required"));

        let session = NewMemoryRecord::promotion_base(
            MemorySource::SessionDistill,
            MemoryPromotionTarget::Session {
                session_id: " session-2 ".to_string(),
                source_span: Some(MemorySourceSpan {
                    start_turn_index: 2,
                    end_turn_index: 4,
                }),
            },
        )
        .expect("session promotion");
        assert_eq!(session.scope, MemoryScope::Session);
        assert_eq!(session.session_id.as_deref(), Some("session-2"));
        assert_eq!(session.thread_id.as_deref(), Some("session-2"));
        assert_eq!(
            session.source_span,
            Some(MemorySourceSpan {
                start_turn_index: 2,
                end_turn_index: 4,
            })
        );
    }

    #[test]
    fn memory_record_file_store_refreshes_cache_when_file_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("records.json");
        let cache = Arc::new(Mutex::new(MemoryRecordCache::default()));
        let first = test_memory_record("memory-cache-1", "First cached record.");
        let second = test_memory_record(
            "memory-cache-2",
            "Second cached record with a longer payload.",
        );

        write_record_file_sync(
            &path,
            &PersistedMemoryRecordFile {
                version: MEMORY_RECORDS_FILE_VERSION,
                records: vec![first.clone()],
            },
        )
        .expect("write first record file");
        let first_map = load_record_map_cached_sync(&path, &cache).expect("load first cache");
        assert_eq!(first_map.get(&first.id), Some(&first));

        write_record_file_sync(
            &path,
            &PersistedMemoryRecordFile {
                version: MEMORY_RECORDS_FILE_VERSION,
                records: vec![second.clone()],
            },
        )
        .expect("write second record file");
        let second_map = load_record_map_cached_sync(&path, &cache).expect("refresh cache");
        assert_eq!(second_map.get(&second.id), Some(&second));
        assert!(!second_map.contains_key(&first.id));
    }

    #[tokio::test]
    async fn memory_store_inserts_and_searches_memory_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = MemoryStore::new(
            Arc::new(MockLlm),
            Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path"))),
        );

        let saved = store
            .insert(NewMemoryRecord::experience(
                "DeepSeek DSML requires a structured parser.",
            ))
            .await
            .expect("insert memory");
        assert!(saved.id.starts_with("memory-"));
        assert_eq!(saved.title, "DeepSeek DSML requires a structured parser");
        assert_eq!(saved.labels, vec![MemoryLabel::Experience]);

        let hits = store
            .search("DSML parser", 8)
            .await
            .expect("search memories");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.id, saved.id);
        assert_eq!(
            hits[0].record.content,
            "DeepSeek DSML requires a structured parser."
        );
        assert_eq!(
            store.get(&saved.id).await.expect("get saved memory"),
            Some(saved)
        );
    }

    #[tokio::test]
    async fn memory_store_uses_separate_embedding_backend_for_vector_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = MemoryStore::new_with_embedding_backend(
            Arc::new(FailingLlmBackend),
            Arc::new(FixedEmbeddingBackend),
            Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path"))),
        );

        let saved = store
            .insert(NewMemoryRecord::experience(
                "DeepSeek DSML requires a structured parser.",
            ))
            .await
            .expect("insert memory with separate embedding backend");
        let hits = store
            .search("structured parser", 8)
            .await
            .expect("search memories with separate embedding backend");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.id, saved.id);
    }

    #[tokio::test]
    async fn memory_store_clamps_importance_and_defaults_labels() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = MemoryStore::new(
            Arc::new(MockLlm),
            Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path"))),
        );

        let saved = store
            .insert(NewMemoryRecord {
                title: Some("".to_string()),
                content: "A durable fact".to_string(),
                labels: Vec::new(),
                importance: 4.0,
                pinned: false,
                source: MemorySource::UserCreated,
                scope: MemoryScope::Workspace,
                session_id: None,
                thread_id: None,
                source_span: None,
            })
            .await
            .expect("insert memory");

        assert_eq!(saved.importance, 1.0);
        assert_eq!(saved.labels, vec![MemoryLabel::Experience]);
        assert_eq!(saved.scope, MemoryScope::Workspace);
    }

    #[tokio::test]
    async fn memory_store_persists_thread_provenance_across_instances() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("lancedb");
        let record_path = temp.path().join("memories").join("records.json");
        let backend = Arc::new(MockLlm);
        let vdb = Arc::new(VectorDB::new(db_path.to_str().expect("utf8 path")));
        let store =
            MemoryStore::new_with_record_path(backend.clone(), vdb.clone(), record_path.clone());

        let saved = store
            .insert(NewMemoryRecord {
                title: Some("Thread decision".to_string()),
                content: "Keep memory retrieval behind MemoryStore.".to_string(),
                labels: vec![MemoryLabel::Decision],
                importance: 0.9,
                pinned: true,
                source: MemorySource::ThreadDistill,
                scope: MemoryScope::Thread,
                session_id: Some("session-123".to_string()),
                thread_id: Some("thread-123".to_string()),
                source_span: Some(MemorySourceSpan {
                    start_turn_index: 2,
                    end_turn_index: 4,
                }),
            })
            .await
            .expect("insert memory");

        let reloaded = MemoryStore::new_with_record_path(backend, vdb, record_path);
        let hits = reloaded
            .search("memory retrieval", 5)
            .await
            .expect("search memories");
        assert_eq!(hits[0].record.id, saved.id);
        assert_eq!(hits[0].record.title, "Thread decision");
        assert_eq!(hits[0].record.labels, vec![MemoryLabel::Decision]);
        assert_eq!(hits[0].record.importance, 0.9);
        assert!(hits[0].record.pinned);
        assert_eq!(hits[0].record.source, MemorySource::ThreadDistill);
        assert_eq!(hits[0].record.scope, MemoryScope::Thread);
        assert_eq!(hits[0].record.session_id.as_deref(), Some("session-123"));
        assert_eq!(hits[0].record.thread_id.as_deref(), Some("thread-123"));
        assert_eq!(
            hits[0].record.source_span,
            Some(MemorySourceSpan {
                start_turn_index: 2,
                end_turn_index: 4,
            })
        );
    }

    #[test]
    fn memory_record_retention_protects_pinned_user_created_and_high_importance_records() {
        let mut record = test_memory_record("memory-retention-1", "Durable user note.");
        assert!(record.is_protected_from_automatic_cleanup());

        record.source = MemorySource::AgentTurn;
        assert!(!record.is_protected_from_automatic_cleanup());

        record.importance = HIGH_IMPORTANCE_RETENTION_THRESHOLD;
        assert!(record.is_protected_from_automatic_cleanup());

        record.importance = DEFAULT_IMPORTANCE;
        record.pinned = true;
        assert!(record.is_protected_from_automatic_cleanup());
    }

    #[tokio::test]
    async fn memory_store_set_pinned_updates_and_persists_record_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("lancedb");
        let record_path = temp.path().join("memories").join("records.json");
        let backend = Arc::new(MockLlm);
        let vdb = Arc::new(VectorDB::new(db_path.to_str().expect("utf8 path")));
        let store =
            MemoryStore::new_with_record_path(backend.clone(), vdb.clone(), record_path.clone());

        let saved = store
            .insert(NewMemoryRecord::experience(
                "Pinned records survive automatic cleanup.",
            ))
            .await
            .expect("insert memory");
        assert!(!saved.pinned);
        assert!(!saved.is_protected_from_automatic_cleanup());

        let pinned = store.set_pinned(&saved.id, true).await.expect("pin memory");
        assert!(pinned.pinned);
        assert!(pinned.is_protected_from_automatic_cleanup());

        let reloaded = MemoryStore::new_with_record_path(backend, vdb, record_path);
        let persisted = reloaded
            .get(&saved.id)
            .await
            .expect("get memory")
            .expect("persisted memory");
        assert!(persisted.pinned);
        assert!(persisted.is_protected_from_automatic_cleanup());
    }

    #[tokio::test]
    async fn memory_store_updates_record_and_refreshes_search_index() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("lancedb");
        let record_path = temp.path().join("memories").join("records.json");
        let backend = Arc::new(MockLlm);
        let vdb = Arc::new(VectorDB::new(db_path.to_str().expect("utf8 path")));
        let store =
            MemoryStore::new_with_record_path(backend.clone(), vdb.clone(), record_path.clone());

        let saved = store
            .insert(NewMemoryRecord::experience("Old parser note."))
            .await
            .expect("insert memory");
        let updated = store
            .update(
                &saved.id,
                MemoryRecordPatch {
                    title: Some("DSML parser decision".to_string()),
                    content: Some(
                        "DeepSeek DSML parser should use a structured parser.".to_string(),
                    ),
                    labels: Some(vec![MemoryLabel::Decision, MemoryLabel::Fact]),
                    importance: Some(0.95),
                    pinned: Some(true),
                    scope: Some(MemoryScope::Workspace),
                    ..Default::default()
                },
            )
            .await
            .expect("update memory");

        assert_eq!(updated.id, saved.id);
        assert_eq!(updated.title, "DSML parser decision");
        assert_eq!(
            updated.content,
            "DeepSeek DSML parser should use a structured parser."
        );
        assert_eq!(
            updated.labels,
            vec![MemoryLabel::Decision, MemoryLabel::Fact]
        );
        assert_eq!(updated.importance, 0.95);
        assert!(updated.pinned);
        assert_eq!(updated.scope, MemoryScope::Workspace);

        let reloaded = MemoryStore::new_with_record_path(backend, vdb, record_path);
        let hits = reloaded
            .search("structured parser", 5)
            .await
            .expect("search");
        assert!(hits.iter().any(|hit| hit.record.id == saved.id));
    }

    #[tokio::test]
    async fn memory_store_refreshes_index_metadata_for_scope_only_update() {
        let temp = tempfile::tempdir().expect("tempdir");
        let backend = Arc::new(MockLlm);
        let vdb = Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path")));
        let store = MemoryStore::new(backend.clone(), vdb.clone());

        let saved = store
            .insert(NewMemoryRecord::experience(
                "Scope-only updates must refresh search index metadata.",
            ))
            .await
            .expect("insert memory");
        store
            .update(
                &saved.id,
                MemoryRecordPatch {
                    scope: Some(MemoryScope::Workspace),
                    ..Default::default()
                },
            )
            .await
            .expect("update scope");

        let metadata_hits = vdb
            .search_with_metadata(
                EXPERIENCES_TABLE,
                LlmBackend::embed(backend.as_ref(), "Scope-only updates")
                    .await
                    .expect("embed"),
                5,
            )
            .await
            .expect("search metadata");
        let metadata = metadata_hits
            .into_iter()
            .map(|(metadata, _score)| metadata)
            .find(|metadata| metadata.id.as_deref() == Some(saved.id.as_str()))
            .expect("updated index metadata");
        assert_eq!(
            metadata.session_id,
            memory_scope_key(&MemoryScope::Workspace)
        );
    }

    #[tokio::test]
    async fn memory_store_delete_hides_stale_lancedb_hits() {
        let temp = tempfile::tempdir().expect("tempdir");
        let backend = Arc::new(MockLlm);
        let vdb = Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path")));
        let store = MemoryStore::new(backend, vdb);

        let saved = store
            .insert(NewMemoryRecord::experience(
                "Deleted memory should not be rehydrated from stale LanceDB rows.",
            ))
            .await
            .expect("insert memory");
        assert_eq!(
            store.delete(&saved.id).await.expect("delete memory"),
            Some(saved.clone())
        );
        assert_eq!(store.get(&saved.id).await.expect("get deleted"), None);

        let hits = store.search("stale LanceDB rows", 5).await.expect("search");
        assert!(
            hits.iter().all(|hit| hit.record.id != saved.id),
            "deleted domain records must not be reconstructed from stale LanceDB index rows"
        );
    }

    #[tokio::test]
    async fn memory_store_list_labels_counts_records_by_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = MemoryStore::new(
            Arc::new(MockLlm),
            Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path"))),
        );

        store
            .insert(NewMemoryRecord {
                title: Some("Workspace decision".to_string()),
                content: "Use the shared MemoryStore API.".to_string(),
                labels: vec![MemoryLabel::Decision, MemoryLabel::Fact],
                importance: 0.7,
                pinned: false,
                source: MemorySource::UserCreated,
                scope: MemoryScope::Workspace,
                session_id: None,
                thread_id: None,
                source_span: None,
            })
            .await
            .expect("insert workspace memory");
        store
            .insert(NewMemoryRecord {
                title: Some("Thread fact".to_string()),
                content: "Thread facts stay scoped to the thread.".to_string(),
                labels: vec![MemoryLabel::Fact],
                importance: 0.4,
                pinned: false,
                source: MemorySource::ThreadDistill,
                scope: MemoryScope::Thread,
                session_id: Some("session-1".to_string()),
                thread_id: Some("thread-1".to_string()),
                source_span: None,
            })
            .await
            .expect("insert thread memory");

        assert_eq!(
            store.list_labels(None).await.expect("all labels"),
            vec![
                MemoryLabelCount {
                    label: MemoryLabel::Fact,
                    count: 2,
                },
                MemoryLabelCount {
                    label: MemoryLabel::Decision,
                    count: 1,
                },
            ]
        );
        assert_eq!(
            store
                .list_labels(Some(MemoryScope::Workspace))
                .await
                .expect("workspace labels"),
            vec![
                MemoryLabelCount {
                    label: MemoryLabel::Decision,
                    count: 1,
                },
                MemoryLabelCount {
                    label: MemoryLabel::Fact,
                    count: 1,
                },
            ]
        );
    }
