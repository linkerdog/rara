impl MemoryStore {
    pub fn new(backend: Arc<dyn LlmBackend>, vdb: Arc<VectorDB>) -> Self {
        let embedding_backend: Arc<dyn EmbeddingBackend> =
            Arc::new(LlmEmbeddingBackend::new(backend.clone()));
        Self::new_with_embedding_backend(backend, embedding_backend, vdb)
    }

    pub fn new_with_embedding_backend(
        llm_backend: Arc<dyn LlmBackend>,
        embedding_backend: Arc<dyn EmbeddingBackend>,
        vdb: Arc<VectorDB>,
    ) -> Self {
        let records = MemoryRecordFileStore::for_vdb_uri(vdb.uri());
        Self {
            llm_backend,
            embedding_backend,
            vdb,
            records,
            observability: global_memory_observability(),
        }
    }

    pub fn new_with_record_path(
        backend: Arc<dyn LlmBackend>,
        vdb: Arc<VectorDB>,
        record_path: PathBuf,
    ) -> Self {
        let embedding_backend: Arc<dyn EmbeddingBackend> =
            Arc::new(LlmEmbeddingBackend::new(backend.clone()));
        Self::new_with_embedding_backend_and_record_path(
            backend,
            embedding_backend,
            vdb,
            record_path,
        )
    }

    pub fn new_with_embedding_backend_and_record_path(
        llm_backend: Arc<dyn LlmBackend>,
        embedding_backend: Arc<dyn EmbeddingBackend>,
        vdb: Arc<VectorDB>,
        record_path: PathBuf,
    ) -> Self {
        Self {
            llm_backend,
            embedding_backend,
            vdb,
            records: MemoryRecordFileStore::new(record_path),
            observability: global_memory_observability(),
        }
    }

    pub(crate) fn backend(&self) -> Arc<dyn LlmBackend> {
        Arc::clone(&self.llm_backend)
    }

    pub async fn insert(&self, input: NewMemoryRecord) -> Result<MemoryRecord> {
        self.insert_with_id(None, input).await
    }

    pub async fn insert_with_id(
        &self,
        id: Option<String>,
        input: NewMemoryRecord,
    ) -> Result<MemoryRecord> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        let content = input.content.trim();
        if content.is_empty() {
            bail!("memory content must not be empty");
        }
        let id = id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| format!("memory-{}", uuid::Uuid::new_v4()));
        let importance = clamp_importance(input.importance);
        let now = unix_timestamp_seconds();
        let record = MemoryRecord {
            id,
            title: input
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| title_from_content(content)),
            content: content.to_string(),
            labels: normalized_labels(input.labels),
            importance,
            pinned: input.pinned,
            source: input.source,
            scope: input.scope,
            session_id: normalized_optional_id(input.session_id),
            thread_id: normalized_optional_id(input.thread_id),
            source_span: input.source_span,
            created_at_unix_seconds: now,
            updated_at_unix_seconds: now,
        };
        let vector = self
            .embedding_backend
            .embed(&record.content, EmbeddingInputKind::Document)
            .await?;
        self.vdb
            .upsert_turn(
                EXPERIENCES_TABLE,
                MemoryMetadata {
                    id: Some(record.id.clone()),
                    session_id: record.index_scope_key(),
                    turn_index: MEMORY_RECORD_INDEX_PLACEHOLDER,
                    text: record.content.clone(),
                },
                vector,
            )
            .await?;
        self.records.upsert(&record).await?;
        Ok(record)
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryRecordSearchHit>> {
        let _timer = self.observability.start_timer(MemoryOperation::Query);
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let query_vector = self
            .embedding_backend
            .embed(query, EmbeddingInputKind::Query)
            .await?;
        self.search_with_embedding_inner(query, query_vector, limit)
            .await
    }

    pub async fn search_with_embedding(
        &self,
        query: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<MemoryRecordSearchHit>> {
        let _timer = self.observability.start_timer(MemoryOperation::Query);
        self.search_with_embedding_inner(query, query_vector, limit)
            .await
    }

    async fn search_with_embedding_inner(
        &self,
        query: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<MemoryRecordSearchHit>> {
        if query.trim().is_empty() || query_vector.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let hits = self
            .vdb
            .hybrid_search_with_metadata(EXPERIENCES_TABLE, query, query_vector, limit)
            .await?;
        let records = self.records.load_map().await?;
        Ok(hits
            .into_iter()
            .filter_map(|hit| {
                memory_record_for_hit(&records, &hit.metadata).map(|record| MemoryRecordSearchHit {
                    record,
                    score: hit.score,
                    vector_distance: hit.vector_distance,
                    fts_score: hit.fts_score,
                })
            })
            .collect())
    }

    pub async fn get(&self, id: &str) -> Result<Option<MemoryRecord>> {
        let _timer = self.observability.start_timer(MemoryOperation::Read);
        self.records.get(id).await
    }

    pub async fn set_pinned(&self, id: &str, pinned: bool) -> Result<MemoryRecord> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        self.records.set_pinned(id, pinned).await
    }

    pub async fn update(&self, id: &str, patch: MemoryRecordPatch) -> Result<MemoryRecord> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        let normalized_patch = normalize_memory_record_patch(patch)?;
        let updated = self.records.update(id, normalized_patch.clone()).await?;
        if patch_requires_index_refresh(&normalized_patch) {
            let vector = self
                .embedding_backend
                .embed(&updated.content, EmbeddingInputKind::Document)
                .await?;
            self.vdb
                .upsert_turn(
                    EXPERIENCES_TABLE,
                    MemoryMetadata {
                        id: Some(updated.id.clone()),
                        session_id: updated.index_scope_key(),
                        turn_index: MEMORY_RECORD_INDEX_PLACEHOLDER,
                        text: updated.content.clone(),
                    },
                    vector,
                )
                .await?;
        }
        Ok(updated)
    }

    pub async fn delete(&self, id: &str) -> Result<Option<MemoryRecord>> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        self.records.delete(id).await
    }

    /// Insert a record without requiring an embedding model.
    /// Writes to JSON only, skips LanceDB vector index.
    pub async fn insert_text_only(&self, input: NewMemoryRecord) -> Result<MemoryRecord> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        let content = input.content.trim();
        if content.is_empty() {
            bail!("memory content must not be empty");
        }
        let id = format!("memory-{}", uuid::Uuid::new_v4());
        let importance = clamp_importance(input.importance);
        let now = unix_timestamp_seconds();
        let record = MemoryRecord {
            id,
            title: input
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| title_from_content(content)),
            content: content.to_string(),
            labels: normalized_labels(input.labels),
            importance,
            pinned: input.pinned,
            source: input.source,
            scope: input.scope,
            session_id: normalized_optional_id(input.session_id),
            thread_id: normalized_optional_id(input.thread_id),
            source_span: input.source_span,
            created_at_unix_seconds: now,
            updated_at_unix_seconds: now,
        };
        self.records.upsert(&record).await?;
        Ok(record)
    }

    /// Returns the most recent records for a scope. No embedding required.
    pub async fn list_recent(
        &self,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let _timer = self.observability.start_timer(MemoryOperation::Read);
        let records_map = self.records.load_map().await?;
        let mut records: Vec<_> = records_map
            .values()
            .filter(|r| scope.as_ref().is_none_or(|s| &r.scope == s))
            .cloned()
            .collect();
        records.sort_by(|a, b| b.created_at_unix_seconds.cmp(&a.created_at_unix_seconds));
        records.truncate(limit);
        Ok(records)
    }

    pub async fn list_labels(&self, scope: Option<MemoryScope>) -> Result<Vec<MemoryLabelCount>> {
        let _timer = self.observability.start_timer(MemoryOperation::Read);
        let records = self.records.load_map().await?;
        Ok(list_label_counts(records.values(), scope.as_ref()))
    }

    pub async fn record_count(&self) -> Result<usize> {
        let _timer = self.observability.start_timer(MemoryOperation::Read);
        Ok(self.records.load_map().await?.len())
    }
}

