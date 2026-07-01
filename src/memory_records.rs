impl MemoryRecord {
    fn index_scope_key(&self) -> String {
        match self.scope {
            MemoryScope::Thread => self
                .thread_id
                .clone()
                .or_else(|| self.session_id.clone())
                .unwrap_or_else(|| memory_scope_key(&self.scope).to_string()),
            MemoryScope::Session => self
                .session_id
                .clone()
                .or_else(|| self.thread_id.clone())
                .unwrap_or_else(|| memory_scope_key(&self.scope).to_string()),
            MemoryScope::User | MemoryScope::Workspace | MemoryScope::Project => {
                memory_scope_key(&self.scope).to_string()
            }
        }
    }

    /// Reserved for automatic memory cleanup once retention enforcement is
    /// wired; see docs/journal/2026-05-05-memory-retention.md.
    #[allow(dead_code)]
    pub fn is_protected_from_automatic_cleanup(&self) -> bool {
        self.pinned
            || self.source == MemorySource::UserCreated
            || self.importance >= HIGH_IMPORTANCE_RETENTION_THRESHOLD
    }
}

impl From<MemoryMetadata> for MemoryRecord {
    fn from(metadata: MemoryMetadata) -> Self {
        let session_id = metadata.session_id.clone();
        let turn_index = metadata.turn_index;
        let now = unix_timestamp_seconds();
        Self {
            id: metadata.id.unwrap_or_else(|| {
                format!("legacy-{}-{}", metadata.session_id, metadata.turn_index)
            }),
            title: title_from_content(&metadata.text),
            content: metadata.text,
            labels: vec![MemoryLabel::Experience],
            importance: DEFAULT_IMPORTANCE,
            pinned: false,
            source: MemorySource::AgentTurn,
            scope: memory_scope_from_key(&metadata.session_id),
            session_id: Some(session_id),
            thread_id: None,
            source_span: Some(MemorySourceSpan {
                start_turn_index: turn_index,
                end_turn_index: turn_index,
            }),
            created_at_unix_seconds: now,
            updated_at_unix_seconds: now,
        }
    }
}

#[derive(Debug, Clone)]
struct MemoryRecordFileStore {
    path: PathBuf,
    lock_path: PathBuf,
    cache: Arc<Mutex<MemoryRecordCache>>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PersistedMemoryRecordFile {
    version: u32,
    records: Vec<MemoryRecord>,
}

impl Default for PersistedMemoryRecordFile {
    fn default() -> Self {
        Self {
            version: MEMORY_RECORDS_FILE_VERSION,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
struct MemoryRecordCache {
    state: Option<MemoryRecordFileState>,
    records: HashMap<String, MemoryRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemoryRecordFileState {
    Missing,
    Present {
        modified_at: Option<SystemTime>,
        len: u64,
    },
}

impl MemoryRecordFileStore {
    fn new(path: PathBuf) -> Self {
        Self {
            lock_path: path.with_extension("json.lock"),
            path,
            cache: Arc::new(Mutex::new(MemoryRecordCache::default())),
        }
    }

    fn for_vdb_uri(uri: &str) -> Self {
        Self::new(default_record_path_for_vdb_uri(uri))
    }

    async fn upsert(&self, record: &MemoryRecord) -> Result<()> {
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let cache = self.cache.clone();
        let record = record.clone();
        tokio::task::spawn_blocking(move || upsert_record_sync(path, lock_path, cache, record))
            .await
            .context("join memory record persistence task")?
    }

    async fn load_map(&self) -> Result<HashMap<String, MemoryRecord>> {
        let path = self.path.clone();
        let cache = self.cache.clone();
        tokio::task::spawn_blocking(move || load_record_map_cached_sync(&path, &cache))
            .await
            .context("join memory record load task")?
    }

    async fn get(&self, id: &str) -> Result<Option<MemoryRecord>> {
        let path = self.path.clone();
        let cache = self.cache.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || get_record_cached_sync(&path, &cache, &id))
            .await
            .context("join memory record get task")?
    }

    /// Reserved for the durable memory pinning API; see
    /// docs/features/memory-records.md.
    #[allow(dead_code)]
    async fn set_pinned(&self, id: &str, pinned: bool) -> Result<MemoryRecord> {
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let cache = self.cache.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            update_record_sync(path, lock_path, cache, &id, |record| {
                if record.pinned != pinned {
                    record.pinned = pinned;
                    record.updated_at_unix_seconds = unix_timestamp_seconds();
                    true
                } else {
                    false
                }
            })
        })
        .await
        .context("join memory record pin update task")?
    }

    async fn update(&self, id: &str, patch: MemoryRecordPatch) -> Result<MemoryRecord> {
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let cache = self.cache.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            update_record_sync(path, lock_path, cache, &id, |record| {
                apply_memory_record_patch(record, patch)
            })
        })
        .await
        .context("join memory record update task")?
    }

    async fn delete(&self, id: &str) -> Result<Option<MemoryRecord>> {
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let cache = self.cache.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || delete_record_sync(path, lock_path, cache, &id))
            .await
            .context("join memory record delete task")?
    }
}
