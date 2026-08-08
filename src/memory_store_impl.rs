impl MemoryStore {
    #[cfg(test)]
    pub fn new(backend: Arc<dyn LlmBackend>, handle: Arc<MemoryHandle>) -> Self {
        Self::new_with_handle(backend, handle)
    }

    pub fn new_with_handle(
        llm_backend: Arc<dyn LlmBackend>,
        handle: Arc<MemoryHandle>,
    ) -> Self {
        let records = MemoryRecordFileStore::for_memory_handle_uri(handle.uri());
        Self {
            llm_backend,
            records,
            observability: global_memory_observability(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_record_path(
        backend: Arc<dyn LlmBackend>,
        handle: Arc<MemoryHandle>,
        record_path: PathBuf,
    ) -> Self {
        Self::new_with_handle_and_record_path(backend, handle, record_path)
    }

    #[cfg(test)]
    pub(crate) fn new_with_handle_and_record_path(
        llm_backend: Arc<dyn LlmBackend>,
        _handle: Arc<MemoryHandle>,
        record_path: PathBuf,
    ) -> Self {
        Self {
            llm_backend,
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

    /// Shared record-construction logic for `insert_with_id` and `insert_text_only`.
    /// Builds a `MemoryRecord` from input but does NOT persist it.
    fn build_memory_record(id: String, input: NewMemoryRecord) -> Result<MemoryRecord> {
        let content = input.content.trim();
        if content.is_empty() {
            bail!("memory content must not be empty");
        }
        let importance = clamp_importance(input.importance);
        let now = unix_timestamp_seconds();
        Ok(MemoryRecord {
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
        })
    }

    pub async fn insert_with_id(
        &self,
        id: Option<String>,
        input: NewMemoryRecord,
    ) -> Result<MemoryRecord> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        let id = id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| format!("memory-{}", uuid::Uuid::new_v4()));
        let record = Self::build_memory_record(id, input)?;
        self.records.upsert(&record).await?;
        Ok(record)
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryRecordSearchHit>> {
        let _timer = self.observability.start_timer(MemoryOperation::Query);
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        self.search_records_text(query, limit).await
    }

    async fn search_records_text(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecordSearchHit>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let records = self.records.load_map().await?;
        let query_terms = normalized_search_terms(query);
        if query_terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = records
            .into_values()
            .filter_map(|record| {
                text_match_score(&record, &query_terms).map(|score| MemoryRecordSearchHit {
                    record,
                    score,
                    vector_distance: None,
                    fts_score: None,
                })
            })
            .collect::<Vec<_>>();
        sort_memory_search_hits(&mut hits);
        hits.truncate(limit);
        Ok(hits)
    }

    pub async fn get(&self, id: &str) -> Result<Option<MemoryRecord>> {
        let _timer = self.observability.start_timer(MemoryOperation::Read);
        self.records.get(id).await
    }

    /// Reserved for the memory-record pinning API documented in
    /// docs/features/memory-records.md.
    #[allow(dead_code)]
    pub async fn set_pinned(&self, id: &str, pinned: bool) -> Result<MemoryRecord> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        self.records.set_pinned(id, pinned).await
    }

    pub async fn update(&self, id: &str, patch: MemoryRecordPatch) -> Result<MemoryRecord> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        let normalized_patch = normalize_memory_record_patch(patch)?;
        self.records.update(id, normalized_patch).await
    }

    pub async fn delete(&self, id: &str) -> Result<Option<MemoryRecord>> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        self.records.delete(id).await
    }

    /// Insert a record without requiring an embedding model.
    /// Writes to JSON only, without local vector indexing.
    pub async fn insert_text_only(&self, input: NewMemoryRecord) -> Result<MemoryRecord> {
        let _timer = self.observability.start_timer(MemoryOperation::Write);
        let record = Self::build_memory_record(
            format!("memory-{}", uuid::Uuid::new_v4()),
            input,
        )?;
        self.records.upsert(&record).await?;
        Ok(record)
    }

    /// Returns the most recent records for a scope. No embedding required.
    /// Reserved for the memory-record listing API documented in
    /// docs/features/memory-records.md.
    #[allow(dead_code)]
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
        records.sort_by_key(|record| std::cmp::Reverse(record.created_at_unix_seconds));
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
