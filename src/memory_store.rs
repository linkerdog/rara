use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rara_persistence::atomic_file;
use rara_persistence::file_lock::AdvisoryFileLock;

use crate::llm::LlmBackend;
use crate::vectordb::{MemoryMetadata, VectorDB};

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
    FileImport,
    ProtocolWrite,
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
    backend: Arc<dyn LlmBackend>,
    vdb: Arc<VectorDB>,
    records: MemoryRecordFileStore,
}

impl MemoryStore {
    pub fn new(backend: Arc<dyn LlmBackend>, vdb: Arc<VectorDB>) -> Self {
        let records = MemoryRecordFileStore::for_vdb_uri(vdb.uri());
        Self {
            backend,
            vdb,
            records,
        }
    }

    pub fn new_with_record_path(
        backend: Arc<dyn LlmBackend>,
        vdb: Arc<VectorDB>,
        record_path: PathBuf,
    ) -> Self {
        Self {
            backend,
            vdb,
            records: MemoryRecordFileStore::new(record_path),
        }
    }

    pub(crate) fn backend(&self) -> Arc<dyn LlmBackend> {
        Arc::clone(&self.backend)
    }

    pub async fn insert(&self, input: NewMemoryRecord) -> Result<MemoryRecord> {
        self.insert_with_id(None, input).await
    }

    pub async fn insert_with_id(
        &self,
        id: Option<String>,
        input: NewMemoryRecord,
    ) -> Result<MemoryRecord> {
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
        let vector = self.backend.embed(&record.content).await?;
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
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let query_vector = self.backend.embed(query).await?;
        self.search_with_embedding(query, query_vector, limit).await
    }

    pub async fn search_with_embedding(
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
        self.records.get(id).await
    }

    pub async fn set_pinned(&self, id: &str, pinned: bool) -> Result<MemoryRecord> {
        self.records.set_pinned(id, pinned).await
    }

    pub async fn update(&self, id: &str, patch: MemoryRecordPatch) -> Result<MemoryRecord> {
        let normalized_patch = normalize_memory_record_patch(patch)?;
        let updated = self.records.update(id, normalized_patch.clone()).await?;
        if patch_requires_index_refresh(&normalized_patch) {
            let vector = self.backend.embed(&updated.content).await?;
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
        self.records.delete(id).await
    }

    pub async fn list_labels(&self, scope: Option<MemoryScope>) -> Result<Vec<MemoryLabelCount>> {
        let records = self.records.load_map().await?;
        Ok(list_label_counts(records.values(), scope.as_ref()))
    }

    pub async fn record_count(&self) -> Result<usize> {
        Ok(self.records.load_map().await?.len())
    }
}

impl MemoryRecord {
    fn index_scope_key(&self) -> String {
        self.session_id
            .clone()
            .or_else(|| self.thread_id.clone())
            .unwrap_or_else(|| memory_scope_key(&self.scope).to_string())
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

fn default_record_path_for_vdb_uri(uri: &str) -> PathBuf {
    let db_path = PathBuf::from(uri);
    if db_path.file_name().and_then(|value| value.to_str()) == Some("lancedb") {
        return db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("memories")
            .join("records.json");
    }
    db_path.join("memory_records.json")
}

fn upsert_record_sync(
    path: PathBuf,
    lock_path: PathBuf,
    cache: Arc<Mutex<MemoryRecordCache>>,
    record: MemoryRecord,
) -> Result<()> {
    let _lock = AdvisoryFileLock::acquire(lock_path)?;
    let mut file = PersistedMemoryRecordFile {
        records: load_records_sync(&path)?,
        ..Default::default()
    };
    if let Some(existing) = file.records.iter_mut().find(|item| item.id == record.id) {
        *existing = record;
    } else {
        file.records.push(record);
    }
    write_record_file_sync(&path, &file)?;
    let records = record_map_from_records(file.records);
    let state = record_file_state(&path)?;
    update_record_cache(&cache, state, records);
    Ok(())
}

fn update_record_sync<F>(
    path: PathBuf,
    lock_path: PathBuf,
    cache: Arc<Mutex<MemoryRecordCache>>,
    id: &str,
    update: F,
) -> Result<MemoryRecord>
where
    F: FnOnce(&mut MemoryRecord) -> bool,
{
    let _lock = AdvisoryFileLock::acquire(lock_path)?;
    let mut file = PersistedMemoryRecordFile {
        records: load_records_sync(&path)?,
        ..Default::default()
    };
    let Some(record) = file.records.iter_mut().find(|item| item.id == id) else {
        bail!("memory record not found: {id}");
    };
    let changed = update(record);
    let updated = record.clone();
    if changed {
        write_record_file_sync(&path, &file)?;
        let records = record_map_from_records(file.records);
        let state = record_file_state(&path)?;
        update_record_cache(&cache, state, records);
    }
    Ok(updated)
}

fn delete_record_sync(
    path: PathBuf,
    lock_path: PathBuf,
    cache: Arc<Mutex<MemoryRecordCache>>,
    id: &str,
) -> Result<Option<MemoryRecord>> {
    let _lock = AdvisoryFileLock::acquire(lock_path)?;
    let mut file = PersistedMemoryRecordFile {
        records: load_records_sync(&path)?,
        ..Default::default()
    };
    let Some(index) = file.records.iter().position(|record| record.id == id) else {
        return Ok(None);
    };
    let deleted = file.records.remove(index);
    write_record_file_sync(&path, &file)?;
    let records = record_map_from_records(file.records);
    let state = record_file_state(&path)?;
    update_record_cache(&cache, state, records);
    Ok(Some(deleted))
}

fn load_record_map_cached_sync(
    path: &Path,
    cache: &Arc<Mutex<MemoryRecordCache>>,
) -> Result<HashMap<String, MemoryRecord>> {
    let state = record_file_state(path)?;
    {
        let cache = cache.lock().expect("memory record cache lock poisoned");
        if cache.state == Some(state) {
            return Ok(cache.records.clone());
        }
    }

    let records = record_map_from_records(load_records_sync(path)?);
    update_record_cache(cache, state, records.clone());
    Ok(records)
}

fn get_record_cached_sync(
    path: &Path,
    cache: &Arc<Mutex<MemoryRecordCache>>,
    id: &str,
) -> Result<Option<MemoryRecord>> {
    Ok(load_record_map_cached_sync(path, cache)?.get(id).cloned())
}

fn update_record_cache(
    cache: &Arc<Mutex<MemoryRecordCache>>,
    state: MemoryRecordFileState,
    records: HashMap<String, MemoryRecord>,
) {
    let mut cache = cache.lock().expect("memory record cache lock poisoned");
    cache.state = Some(state);
    cache.records = records;
}

fn record_map_from_records(records: Vec<MemoryRecord>) -> HashMap<String, MemoryRecord> {
    records
        .into_iter()
        .map(|record| (record.id.clone(), record))
        .collect()
}

fn record_file_state(path: &Path) -> Result<MemoryRecordFileState> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(MemoryRecordFileState::Present {
            modified_at: metadata.modified().ok(),
            len: metadata.len(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(MemoryRecordFileState::Missing)
        }
        Err(err) => Err(err).with_context(|| format!("stat memory records {}", path.display())),
    }
}

fn load_records_sync(path: &Path) -> Result<Vec<MemoryRecord>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("read memory records {}", path.display()));
        }
    };
    let reader = BufReader::new(file);
    match serde_json::from_reader::<_, PersistedMemoryRecordEnvelope>(reader) {
        Ok(PersistedMemoryRecordEnvelope::Versioned(file)) => Ok(file.records),
        Ok(PersistedMemoryRecordEnvelope::Legacy(records)) => Ok(records),
        Err(err) if err.is_eof() => Ok(Vec::new()),
        Err(err) => Err(err).with_context(|| format!("parse memory records {}", path.display())),
    }
}

fn write_record_file_sync(path: &Path, file: &PersistedMemoryRecordFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create memory records dir {}", parent.display()))?;
    }
    let tmp_path = path.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
    let tmp_file = File::create(&tmp_path)
        .with_context(|| format!("create memory records temp file {}", tmp_path.display()))?;
    let mut writer = BufWriter::new(tmp_file);
    serde_json::to_writer(&mut writer, file)
        .with_context(|| format!("serialize memory records {}", tmp_path.display()))?;
    writer
        .flush()
        .with_context(|| format!("flush memory records temp file {}", tmp_path.display()))?;
    if let Err(err) = atomic_file::replace_file(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err).with_context(|| format!("replace memory records {}", path.display()));
    }
    Ok(())
}

fn memory_record_for_hit(
    records: &HashMap<String, MemoryRecord>,
    metadata: &MemoryMetadata,
) -> Option<MemoryRecord> {
    match metadata.id.as_ref() {
        Some(id) => records.get(id).cloned(),
        None => Some(MemoryRecord::from(metadata.clone())),
    }
}

fn normalize_memory_record_patch(mut patch: MemoryRecordPatch) -> Result<MemoryRecordPatch> {
    if let Some(title) = patch.title.take() {
        patch.title = Some(title.trim().to_string());
    }
    if let Some(content) = patch.content.take() {
        let content = content.trim().to_string();
        if content.is_empty() {
            bail!("memory content must not be empty");
        }
        patch.content = Some(content);
    }
    if let Some(labels) = patch.labels.take() {
        patch.labels = Some(normalized_labels(labels));
    }
    if let Some(importance) = patch.importance.take() {
        patch.importance = Some(clamp_importance(importance));
    }
    if let Some(session_id) = patch.session_id.take() {
        patch.session_id = Some(normalized_optional_id(session_id));
    }
    if let Some(thread_id) = patch.thread_id.take() {
        patch.thread_id = Some(normalized_optional_id(thread_id));
    }
    Ok(patch)
}

fn patch_requires_index_refresh(patch: &MemoryRecordPatch) -> bool {
    patch.content.is_some()
        || patch.scope.is_some()
        || patch.session_id.is_some()
        || patch.thread_id.is_some()
}

fn apply_memory_record_patch(record: &mut MemoryRecord, patch: MemoryRecordPatch) -> bool {
    let before = record.clone();
    if let Some(title) = patch.title.filter(|title| !title.is_empty()) {
        record.title = title;
    }
    if let Some(content) = patch.content {
        record.content = content;
    }
    if let Some(labels) = patch.labels {
        record.labels = labels;
    }
    if let Some(importance) = patch.importance {
        record.importance = importance;
    }
    if let Some(pinned) = patch.pinned {
        record.pinned = pinned;
    }
    if let Some(scope) = patch.scope {
        record.scope = scope;
    }
    if let Some(session_id) = patch.session_id {
        record.session_id = session_id;
    }
    if let Some(thread_id) = patch.thread_id {
        record.thread_id = thread_id;
    }
    if let Some(source_span) = patch.source_span {
        record.source_span = source_span;
    }
    if *record != before {
        record.updated_at_unix_seconds = unix_timestamp_seconds();
        true
    } else {
        false
    }
}

fn list_label_counts<'a>(
    records: impl IntoIterator<Item = &'a MemoryRecord>,
    scope: Option<&MemoryScope>,
) -> Vec<MemoryLabelCount> {
    let mut counts = HashMap::<MemoryLabel, usize>::new();
    for record in records {
        if scope.is_some_and(|scope| &record.scope != scope) {
            continue;
        }
        for label in &record.labels {
            *counts.entry(label.clone()).or_default() += 1;
        }
    }
    let mut counts = counts
        .into_iter()
        .map(|(label, count)| MemoryLabelCount { label, count })
        .collect::<Vec<_>>();
    counts.sort_by(|left, right| {
        right.count.cmp(&left.count).then_with(|| {
            memory_label_sort_key(&left.label).cmp(memory_label_sort_key(&right.label))
        })
    });
    counts
}

fn memory_label_sort_key(label: &MemoryLabel) -> &'static str {
    match label {
        MemoryLabel::Insight => "insight",
        MemoryLabel::Decision => "decision",
        MemoryLabel::Fact => "fact",
        MemoryLabel::Procedure => "procedure",
        MemoryLabel::Experience => "experience",
    }
}

fn normalized_labels(labels: Vec<MemoryLabel>) -> Vec<MemoryLabel> {
    if labels.is_empty() {
        return vec![MemoryLabel::Experience];
    }
    labels
}

fn normalized_optional_id(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clamp_importance(importance: f32) -> f32 {
    if importance.is_nan() {
        return DEFAULT_IMPORTANCE;
    }
    importance.clamp(0.1, 1.0)
}

fn memory_scope_key(scope: &MemoryScope) -> &'static str {
    match scope {
        MemoryScope::User => "user",
        MemoryScope::Workspace => "workspace",
        MemoryScope::Project => "project",
        MemoryScope::Thread => "thread",
        MemoryScope::Session => "session",
    }
}

fn memory_scope_from_key(value: &str) -> MemoryScope {
    match value {
        "user" => MemoryScope::User,
        "workspace" => MemoryScope::Workspace,
        "thread" => MemoryScope::Thread,
        "session" => MemoryScope::Session,
        _ => MemoryScope::Project,
    }
}

fn title_from_content(content: &str) -> String {
    let first_line = content
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Memory");
    let title = first_line
        .split_terminator(['.', '!', '?'])
        .next()
        .unwrap_or(first_line)
        .trim();
    truncate_chars(title, 80)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlm;

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
                backend.embed("Scope-only updates").await.expect("embed"),
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
}
