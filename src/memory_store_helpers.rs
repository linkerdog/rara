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

fn required_memory_id(value: String, field: &str) -> Result<String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        bail!("{field} is required for scoped memory promotion");
    }
    Ok(value)
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

/// Compute the new content string after applying `patch` to `record`.
/// Returns the record's existing content when no content-related fields
/// are set in the patch.
pub(crate) fn apply_patch_to_content(record: &MemoryRecord, patch: &MemoryRecordPatch) -> String {
    match &patch.content {
        Some(c) if !c.trim().is_empty() => c.clone(),
        Some(_) => record.content.clone(),
        None => match &patch.title {
            Some(t) => format!("{}\n\n{}", t, record.content),
            None => record.content.clone(),
        },
    }
}

