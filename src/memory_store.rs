use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rara_memory::vectordb::{MemoryMetadata, VectorDB};
use rara_observability::{MemoryObservability, MemoryOperation, global_memory_observability};
use rara_persistence::atomic_file;
use rara_persistence::file_lock::AdvisoryFileLock;

use crate::llm::{EmbeddingBackend, EmbeddingInputKind, LlmBackend, LlmEmbeddingBackend};

const EXPERIENCES_TABLE: &str = "experiences";
const DEFAULT_IMPORTANCE: f32 = 0.5;
const HIGH_IMPORTANCE_RETENTION_THRESHOLD: f32 = 0.8;
const MEMORY_RECORD_INDEX_PLACEHOLDER: u32 = 0;
const MEMORY_RECORDS_FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLabel {
    Insight,
    Decision,
    Fact,
    Procedure,
    Experience,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    AgentTurn,
    UserCreated,
    ThreadDistill,
    SessionDistill,
    FileImport,
    ProtocolWrite,
    AutoMemory,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Workspace,
    Project,
    Thread,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemorySourceSpan {
    pub start_turn_index: u32,
    pub end_turn_index: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub title: String,
    pub content: String,
    pub labels: Vec<MemoryLabel>,
    pub importance: f32,
    #[serde(default)]
    pub pinned: bool,
    pub source: MemorySource,
    pub scope: MemoryScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_span: Option<MemorySourceSpan>,
    pub created_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct NewMemoryRecord {
    pub title: Option<String>,
    pub content: String,
    pub labels: Vec<MemoryLabel>,
    pub importance: f32,
    pub pinned: bool,
    pub source: MemorySource,
    pub scope: MemoryScope,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub source_span: Option<MemorySourceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryPromotionTarget {
    Workspace {
        session_id: Option<String>,
        thread_id: Option<String>,
    },
    Thread {
        session_id: Option<String>,
        thread_id: String,
        source_span: Option<MemorySourceSpan>,
    },
    Session {
        session_id: String,
        source_span: Option<MemorySourceSpan>,
    },
}

impl NewMemoryRecord {
    pub fn experience(content: impl Into<String>) -> Self {
        Self {
            title: None,
            content: content.into(),
            labels: vec![MemoryLabel::Experience],
            importance: DEFAULT_IMPORTANCE,
            pinned: false,
            source: MemorySource::AgentTurn,
            scope: MemoryScope::Project,
            session_id: None,
            thread_id: None,
            source_span: None,
        }
    }

    pub fn promotion_base(source: MemorySource, target: MemoryPromotionTarget) -> Result<Self> {
        let (scope, session_id, thread_id, source_span) = match target {
            MemoryPromotionTarget::Workspace {
                session_id,
                thread_id,
            } => (
                MemoryScope::Workspace,
                normalized_optional_id(session_id),
                normalized_optional_id(thread_id),
                None,
            ),
            MemoryPromotionTarget::Thread {
                session_id,
                thread_id,
                source_span,
            } => (
                MemoryScope::Thread,
                normalized_optional_id(session_id),
                Some(required_memory_id(thread_id, "thread_id")?),
                source_span,
            ),
            MemoryPromotionTarget::Session {
                session_id,
                source_span,
            } => {
                let session_id = required_memory_id(session_id, "session_id")?;
                (
                    MemoryScope::Session,
                    Some(session_id.clone()),
                    Some(session_id),
                    source_span,
                )
            }
        };

        Ok(Self {
            title: None,
            content: String::new(),
            labels: vec![MemoryLabel::Experience],
            importance: 0.6,
            pinned: false,
            source,
            scope,
            session_id,
            thread_id,
            source_span,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryRecordPatch {
    pub title: Option<String>,
    pub content: Option<String>,
    pub labels: Option<Vec<MemoryLabel>>,
    pub importance: Option<f32>,
    pub pinned: Option<bool>,
    pub scope: Option<MemoryScope>,
    pub session_id: Option<Option<String>>,
    pub thread_id: Option<Option<String>>,
    pub source_span: Option<Option<MemorySourceSpan>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLabelCount {
    pub label: MemoryLabel,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRecordSearchHit {
    pub record: MemoryRecord,
    pub score: f32,
    pub vector_distance: Option<f32>,
    pub fts_score: Option<f32>,
}

pub struct MemoryStore {
    llm_backend: Arc<dyn LlmBackend>,
    embedding_backend: Arc<dyn EmbeddingBackend>,
    vdb: Arc<VectorDB>,
    records: MemoryRecordFileStore,
    observability: Arc<MemoryObservability>,
}

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

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum PersistedMemoryRecordEnvelope {
    Versioned(PersistedMemoryRecordFile),
    Legacy(Vec<MemoryRecord>),
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

include!("memory_store_helpers.rs");
#[cfg(test)]
mod tests {
    include!("memory_store_tests.rs");
}
