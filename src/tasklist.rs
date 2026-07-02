use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs2::FileExt;
use rara_persistence::atomic_file;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_TASK_LIST_ID: &str = "default";

#[derive(Debug, Clone)]
pub struct TaskListStore {
    root: PathBuf,
}

impl TaskListStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn list_tasks(&self, task_list_id: &str) -> Result<Vec<TaskRecord>> {
        let task_list_dir = self.task_list_dir(task_list_id);
        if !is_real_directory(&task_list_dir) {
            return Ok(Vec::new());
        }

        let mut tasks = Vec::new();
        for entry in fs::read_dir(&task_list_dir)
            .with_context(|| format!("read task list directory {}", task_list_dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("read task list directory entry {}", task_list_dir.display())
            })?;
            let path = entry.path();
            let Some(task_id) = task_id_from_path(&path) else {
                continue;
            };
            match read_task_file(&path, &task_id) {
                Ok(task) => tasks.push(task),
                Err(err) => log::warn!("Failed to read task file {}: {err}", path.display()),
            }
        }
        sort_tasks_by_id(&mut tasks);
        Ok(tasks)
    }

    pub fn get_task(&self, task_list_id: &str, task_id: &str) -> Result<Option<TaskRecord>> {
        let task_id = task_id.trim();
        if !is_valid_task_id(task_id) {
            return Ok(None);
        }

        let path = self
            .task_list_dir(task_list_id)
            .join(format!("{task_id}.json"));
        if !is_real_file(&path) {
            return Ok(None);
        }
        read_task_file(&path, task_id).map(Some)
    }

    pub fn create_task(&self, task_list_id: &str, input: NewTaskRecord) -> Result<TaskRecord> {
        let task_list_dir = self.prepare_task_list_dir(task_list_id)?;
        let lock_file = open_lock_file(&task_list_dir)?;
        lock_file
            .lock_exclusive()
            .with_context(|| format!("lock task list directory {}", task_list_dir.display()))?;

        let result = (|| {
            let next_id = self.next_task_id_in_dir(&task_list_dir)?;
            let task = TaskRecord {
                id: next_id,
                subject: input.subject,
                description: input.description,
                active_form: input.active_form,
                owner: None,
                status: TaskStatus::Pending,
                revision: 1,
                updated_at: unix_timestamp_secs(),
                blocks: Vec::new(),
                blocked_by: Vec::new(),
                metadata: input.metadata,
            };
            self.write_new_task_file(&task_list_dir, &task)?;
            Ok(task)
        })();

        if let Err(err) = lock_file.unlock() {
            log::warn!(
                "Failed to unlock task list directory {}: {err}",
                task_list_dir.display()
            );
        }
        result
    }

    pub fn update_task(
        &self,
        task_list_id: &str,
        task_id: &str,
        update: TaskUpdate,
    ) -> Result<TaskUpdateOutcome> {
        let task_id = task_id.trim();
        if !is_valid_task_id(task_id) {
            anyhow::bail!("invalid task id '{task_id}'");
        }
        let task_list_dir = self.prepare_task_list_dir(task_list_id)?;
        let lock_file = open_lock_file(&task_list_dir)?;
        lock_file
            .lock_exclusive()
            .with_context(|| format!("lock task list directory {}", task_list_dir.display()))?;

        let result = (|| {
            if update.delete {
                let task_path = task_list_dir.join(format!("{task_id}.json"));
                let Some(task) = self.get_task_from_dir(&task_list_dir, task_id)? else {
                    return Ok(TaskUpdateOutcome::not_found(task_id));
                };
                self.remove_known_task_references(&task_list_dir, task_id, &task)?;
                fs::remove_file(&task_path)
                    .with_context(|| format!("delete task file {}", task_path.display()))?;
                sync_parent_dir_best_effort(&task_list_dir);
                return Ok(TaskUpdateOutcome {
                    success: true,
                    task_id: task_id.to_string(),
                    updated_fields: vec!["deleted".to_string()],
                    revision: None,
                    updated_at: None,
                    status_change: None,
                    error: None,
                });
            }

            let Some(mut task) = self.get_task_from_dir(&task_list_dir, task_id)? else {
                return Ok(TaskUpdateOutcome::not_found(task_id));
            };
            if let Some(expected_revision) = update.expected_revision
                && task.revision != expected_revision
            {
                return Ok(TaskUpdateOutcome::stale(
                    task_id,
                    task.revision,
                    task.updated_at,
                ));
            }
            let old_status = task.status.clone();
            let mut updated_fields = Vec::new();

            apply_text_update(
                &mut task.subject,
                update.subject,
                "subject",
                &mut updated_fields,
            );
            apply_text_update(
                &mut task.description,
                update.description,
                "description",
                &mut updated_fields,
            );
            apply_optional_text_update(
                &mut task.active_form,
                update.active_form,
                "activeForm",
                &mut updated_fields,
            );
            if let Some(conflict) = apply_owner_claim_update(
                &mut task.owner,
                update.claim_owner.as_deref(),
                update.release_owner.as_deref(),
                &mut updated_fields,
            ) {
                return Ok(TaskUpdateOutcome::conflict(
                    task_id,
                    task.revision,
                    task.updated_at,
                    conflict,
                ));
            }
            apply_optional_text_update(&mut task.owner, update.owner, "owner", &mut updated_fields);

            if let Some(status) = update.status
                && task.status != status
            {
                task.status = status;
                updated_fields.push("status".to_string());
            }

            if !update.metadata.is_empty() {
                merge_metadata(&mut task.metadata, update.metadata);
                updated_fields.push("metadata".to_string());
            }

            self.apply_dependency_updates(
                &task_list_dir,
                task_id,
                &mut task,
                &update.add_blocks,
                &update.add_blocked_by,
                &mut updated_fields,
            )?;
            if !updated_fields.is_empty() {
                bump_task_revision(&mut task);
                self.write_task_file(&task_list_dir, &task)?;
            }

            updated_fields.sort();
            updated_fields.dedup();
            Ok(TaskUpdateOutcome {
                success: true,
                task_id: task_id.to_string(),
                updated_fields,
                status_change: (old_status != task.status).then_some(StatusChange {
                    from: old_status,
                    to: task.status,
                }),
                revision: Some(task.revision),
                updated_at: Some(task.updated_at),
                error: None,
            })
        })();

        if let Err(err) = lock_file.unlock() {
            log::warn!(
                "Failed to unlock task list directory {}: {err}",
                task_list_dir.display()
            );
        }
        result
    }

    fn task_list_dir(&self, task_list_id: &str) -> PathBuf {
        self.root.join(normalize_task_list_id(task_list_id))
    }

    fn prepare_task_list_dir(&self, task_list_id: &str) -> Result<PathBuf> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create task root directory {}", self.root.display()))?;
        if !is_real_directory(&self.root) {
            anyhow::bail!(
                "task root path {} is not a real directory",
                self.root.display()
            );
        }
        let task_list_dir = self.task_list_dir(task_list_id);
        if task_list_dir.exists() && !is_real_directory(&task_list_dir) {
            anyhow::bail!(
                "task list path {} is not a real directory",
                task_list_dir.display()
            );
        }
        fs::create_dir_all(&task_list_dir)
            .with_context(|| format!("create task list directory {}", task_list_dir.display()))?;
        Ok(task_list_dir)
    }

    fn next_task_id_in_dir(&self, task_list_dir: &Path) -> Result<String> {
        let mut highest_id = 0u64;
        for entry in fs::read_dir(task_list_dir)
            .with_context(|| format!("read task list directory {}", task_list_dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("read task list directory entry {}", task_list_dir.display())
            })?;
            let Some(task_id) = task_id_from_path(&entry.path()) else {
                continue;
            };
            if let Ok(id) = task_id.parse::<u64>() {
                highest_id = highest_id.max(id);
            }
        }
        Ok((highest_id + 1).to_string())
    }

    fn write_new_task_file(&self, task_list_dir: &Path, task: &TaskRecord) -> Result<()> {
        let path = task_list_dir.join(format!("{}.json", task.id));
        if fs::symlink_metadata(&path).is_ok() {
            anyhow::bail!("task file {} already exists", path.display());
        }
        self.write_task_file_at_path(task_list_dir, task, path)
    }

    fn write_task_file(&self, task_list_dir: &Path, task: &TaskRecord) -> Result<()> {
        let path = task_list_dir.join(format!("{}.json", task.id));
        if !is_real_file(&path) {
            anyhow::bail!("task file {} is not a real file", path.display());
        }
        self.write_task_file_at_path(task_list_dir, task, path)
    }

    fn write_task_file_at_path(
        &self,
        task_list_dir: &Path,
        task: &TaskRecord,
        path: PathBuf,
    ) -> Result<()> {
        let tmp_path = task_list_dir.join(format!(".{}.json.tmp-{}", task.id, Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::File::create(&tmp_path)
                .with_context(|| format!("create temporary task file {}", tmp_path.display()))?;
            let content = serde_json::to_vec_pretty(task).context("serialize task")?;
            file.write_all(&content)
                .with_context(|| format!("write temporary task file {}", tmp_path.display()))?;
            file.sync_all()
                .with_context(|| format!("sync temporary task file {}", tmp_path.display()))?;
            atomic_file::replace_file(&tmp_path, &path)
                .with_context(|| format!("replace task file {}", path.display()))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
        result?;
        sync_parent_dir_best_effort(task_list_dir);
        Ok(())
    }

    fn get_task_from_dir(&self, task_list_dir: &Path, task_id: &str) -> Result<Option<TaskRecord>> {
        let path = task_list_dir.join(format!("{task_id}.json"));
        if !is_real_file(&path) {
            return Ok(None);
        }
        read_task_file(&path, task_id).map(Some)
    }

    fn apply_dependency_updates(
        &self,
        task_list_dir: &Path,
        task_id: &str,
        task: &mut TaskRecord,
        add_blocks: &[String],
        add_blocked_by: &[String],
        updated_fields: &mut Vec<String>,
    ) -> Result<()> {
        let mut related_tasks = BTreeMap::new();
        for blocked_id in add_blocks {
            self.ensure_valid_dependency(task_id, blocked_id)?;
            if !related_tasks.contains_key(blocked_id) {
                related_tasks.insert(
                    blocked_id.clone(),
                    self.load_dependency_task(task_list_dir, blocked_id, "blocked")?,
                );
            }
        }
        for blocker_id in add_blocked_by {
            self.ensure_valid_dependency(blocker_id, task_id)?;
            if !related_tasks.contains_key(blocker_id) {
                related_tasks.insert(
                    blocker_id.clone(),
                    self.load_dependency_task(task_list_dir, blocker_id, "blocking")?,
                );
            }
        }

        for blocked_id in add_blocks {
            let blocked = related_tasks
                .get_mut(blocked_id)
                .expect("dependency task was loaded");
            if push_unique(&mut blocked.blocked_by, task_id.to_string()) {
                bump_task_revision(blocked);
            }
            push_unique(&mut task.blocks, blocked_id.clone());
        }
        if !add_blocks.is_empty() {
            updated_fields.push("blocks".to_string());
        }

        for blocker_id in add_blocked_by {
            let blocker = related_tasks
                .get_mut(blocker_id)
                .expect("dependency task was loaded");
            if push_unique(&mut blocker.blocks, task_id.to_string()) {
                bump_task_revision(blocker);
            }
            push_unique(&mut task.blocked_by, blocker_id.clone());
        }
        if !add_blocked_by.is_empty() {
            updated_fields.push("blockedBy".to_string());
        }

        for related_task in related_tasks.values() {
            self.write_task_file(task_list_dir, related_task)?;
        }
        Ok(())
    }

    fn ensure_valid_dependency(&self, blocker_id: &str, blocked_id: &str) -> Result<()> {
        if !is_valid_task_id(blocker_id)
            || !is_valid_task_id(blocked_id)
            || blocker_id == blocked_id
        {
            anyhow::bail!("invalid task dependency '{blocker_id}' -> '{blocked_id}'");
        }
        Ok(())
    }

    fn load_dependency_task(
        &self,
        task_list_dir: &Path,
        task_id: &str,
        role: &str,
    ) -> Result<TaskRecord> {
        self.get_task_from_dir(task_list_dir, task_id)?
            .with_context(|| format!("{role} task '{task_id}' not found"))
    }

    fn remove_known_task_references(
        &self,
        task_list_dir: &Path,
        task_id: &str,
        task: &TaskRecord,
    ) -> Result<()> {
        let mut related_tasks = BTreeMap::new();
        for related_id in task.blocked_by.iter().chain(task.blocks.iter()) {
            if !is_valid_task_id(related_id) || related_id == task_id {
                log::warn!(
                    "Skipping invalid task dependency reference '{related_id}' while deleting task '{task_id}'"
                );
                continue;
            }
            if related_tasks.contains_key(related_id) {
                continue;
            }
            if let Some(related_task) = self.get_task_from_dir(task_list_dir, related_id)? {
                related_tasks.insert(related_id.clone(), related_task);
            }
        }

        for related_task in related_tasks.values_mut() {
            related_task.blocks.retain(|id| id != task_id);
            related_task.blocked_by.retain(|id| id != task_id);
        }
        for related_task in related_tasks.values() {
            self.write_task_file(task_list_dir, related_task)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTaskRecord {
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskUpdate {
    pub expected_revision: Option<u64>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub active_form: Option<Option<String>>,
    pub owner: Option<Option<String>>,
    pub claim_owner: Option<String>,
    pub release_owner: Option<String>,
    pub status: Option<TaskStatus>,
    pub metadata: BTreeMap<String, serde_json::Value>,
    pub add_blocks: Vec<String>,
    pub add_blocked_by: Vec<String>,
    pub delete: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateOutcome {
    pub success: bool,
    pub task_id: String,
    pub updated_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_change: Option<StatusChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TaskUpdateOutcome {
    fn not_found(task_id: &str) -> Self {
        Self {
            success: false,
            task_id: task_id.to_string(),
            updated_fields: Vec::new(),
            revision: None,
            updated_at: None,
            error: Some("Task not found".to_string()),
            status_change: None,
        }
    }

    fn stale(task_id: &str, current_revision: u64, updated_at: u64) -> Self {
        Self {
            success: false,
            task_id: task_id.to_string(),
            updated_fields: Vec::new(),
            revision: Some(current_revision),
            updated_at: Some(updated_at),
            error: Some("Task revision mismatch".to_string()),
            status_change: None,
        }
    }

    fn conflict(task_id: &str, current_revision: u64, updated_at: u64, error: String) -> Self {
        Self {
            success: false,
            task_id: task_id.to_string(),
            updated_fields: Vec::new(),
            revision: Some(current_revision),
            updated_at: Some(updated_at),
            error: Some(error),
            status_change: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusChange {
    pub from: TaskStatus,
    pub to: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: String,
    pub subject: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, alias = "activeForm")]
    pub active_form: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    pub status: TaskStatus,
    #[serde(default)]
    pub revision: u64,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default, alias = "blockedBy")]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

fn apply_text_update(
    target: &mut String,
    update: Option<String>,
    field_name: &str,
    updated_fields: &mut Vec<String>,
) {
    if let Some(update) = update
        && *target != update
    {
        *target = update;
        updated_fields.push(field_name.to_string());
    }
}

fn apply_optional_text_update(
    target: &mut Option<String>,
    update: Option<Option<String>>,
    field_name: &str,
    updated_fields: &mut Vec<String>,
) {
    if let Some(update) = update
        && *target != update
    {
        *target = update;
        updated_fields.push(field_name.to_string());
    }
}

fn apply_owner_claim_update(
    owner: &mut Option<String>,
    claim_owner: Option<&str>,
    release_owner: Option<&str>,
    updated_fields: &mut Vec<String>,
) -> Option<String> {
    if let Some(claim_owner) = claim_owner {
        match owner.as_deref() {
            Some(existing) if existing != claim_owner => {
                return Some(format!("Task already owned by {existing}"));
            }
            Some(_) => {}
            None => {
                *owner = Some(claim_owner.to_string());
                updated_fields.push("owner".to_string());
            }
        }
    }

    if let Some(release_owner) = release_owner {
        match owner.as_deref() {
            Some(existing) if existing != release_owner => {
                return Some(format!("Task is owned by {existing}, not {release_owner}"));
            }
            Some(_) => {
                *owner = None;
                updated_fields.push("owner".to_string());
            }
            None => {}
        }
    }
    None
}

fn bump_task_revision(task: &mut TaskRecord) {
    task.revision = task.revision.saturating_add(1);
    task.updated_at = unix_timestamp_secs();
}

fn merge_metadata(
    target: &mut BTreeMap<String, serde_json::Value>,
    updates: BTreeMap<String, serde_json::Value>,
) {
    for (key, value) in updates {
        if value.is_null() {
            target.remove(&key);
        } else {
            target.insert(key, value);
        }
    }
}

fn push_unique(values: &mut Vec<String>, value: String) -> bool {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
        return true;
    }
    false
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskListEntry {
    pub id: String,
    pub subject: String,
    pub status: TaskStatus,
    pub revision: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub blocked_by: Vec<String>,
}

impl TaskListEntry {
    pub fn from_task(task: &TaskRecord, completed_task_ids: &HashSet<&str>) -> Self {
        let blocked_by = task
            .blocked_by
            .iter()
            .filter(|task_id| !completed_task_ids.contains(task_id.as_str()))
            .cloned()
            .collect();
        Self {
            id: task.id.clone(),
            subject: task.subject.clone(),
            status: task.status.clone(),
            revision: task.revision,
            updated_at: task.updated_at,
            owner: task.owner.clone(),
            blocked_by,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetails {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub revision: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
}

impl From<TaskRecord> for TaskDetails {
    fn from(task: TaskRecord) -> Self {
        Self {
            id: task.id,
            subject: task.subject,
            description: task.description,
            status: task.status,
            revision: task.revision,
            updated_at: task.updated_at,
            owner: task.owner,
            blocks: task.blocks,
            blocked_by: task.blocked_by,
        }
    }
}

pub fn task_list_entries(tasks: &[TaskRecord]) -> Vec<TaskListEntry> {
    let completed_task_ids = tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Completed)
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    tasks
        .iter()
        .map(|task| TaskListEntry::from_task(task, &completed_task_ids))
        .collect()
}

pub fn is_valid_task_id(task_id: &str) -> bool {
    let task_id = task_id.trim();
    !task_id.is_empty()
        && !task_id.contains('/')
        && !task_id.contains('\\')
        && !task_id.contains("..")
        && !Path::new(task_id).is_absolute()
}

fn read_task_file(path: &Path, expected_task_id: &str) -> Result<TaskRecord> {
    let bytes = fs::read(path).with_context(|| format!("read task file {}", path.display()))?;
    let mut task: TaskRecord = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse task file {}", path.display()))?;
    anyhow::ensure!(
        is_valid_task_id(&task.id) && task.id == expected_task_id,
        "task id '{}' does not match file id '{}'",
        task.id,
        expected_task_id
    );
    if task.revision == 0 {
        task.revision = 1;
    }
    Ok(task)
}

fn task_id_from_path(path: &Path) -> Option<String> {
    if !is_real_file(path) {
        return None;
    }
    let file_name = path.file_name()?.to_str()?;
    if file_name.starts_with('.') {
        return None;
    }
    let extension = path.extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case("json") {
        return None;
    }
    let task_id = path.file_stem()?.to_str()?;
    is_valid_task_id(task_id).then(|| task_id.to_string())
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
}

fn is_real_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn open_lock_file(task_list_dir: &Path) -> Result<fs::File> {
    let lock_path = task_list_dir.join(".lock");
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(file) => Ok(file),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            if !is_real_file(&lock_path) {
                anyhow::bail!(
                    "task list lock path {} is not a real file",
                    lock_path.display()
                );
            }
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .with_context(|| format!("open task list lock file {}", lock_path.display()))
        }
        Err(err) => {
            Err(err).with_context(|| format!("create task list lock file {}", lock_path.display()))
        }
    }
}

fn sync_parent_dir_best_effort(path: &Path) {
    if let Ok(dir) = fs::File::open(path)
        && let Err(err) = dir.sync_all()
    {
        log::warn!(
            "Failed to sync task list directory {}: {err}",
            path.display()
        );
    }
}

fn normalize_task_list_id(task_list_id: &str) -> String {
    let normalized = task_list_id
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if normalized.is_empty() {
        DEFAULT_TASK_LIST_ID.to_string()
    } else {
        normalized
    }
}

fn sort_tasks_by_id(tasks: &mut [TaskRecord]) {
    tasks.sort_by(
        |left, right| match (left.id.parse::<u64>(), right.id.parse::<u64>()) {
            (Ok(left_id), Ok(right_id)) => left_id.cmp(&right_id),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            (Err(_), Err(_)) => left.id.cmp(&right.id),
        },
    );
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn list_tasks_reads_sorted_task_files() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "10",
            json!({
                "id": "10",
                "subject": "Later task",
                "status": "pending"
            }),
        );
        write_task(
            &list_dir,
            "2",
            json!({
                "id": "2",
                "subject": "Earlier task",
                "status": "in_progress"
            }),
        );

        let store = TaskListStore::new(temp.path());
        let tasks = store
            .list_tasks(DEFAULT_TASK_LIST_ID)
            .expect("tasks should load");

        assert_eq!(
            tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2", "10"]
        );
    }

    #[test]
    fn create_task_allocates_next_numeric_id_and_writes_pending_task() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "2",
            json!({
                "id": "2",
                "subject": "Existing task",
                "status": "completed"
            }),
        );
        write_task(
            &list_dir,
            "custom",
            json!({
                "id": "custom",
                "subject": "Non-numeric task",
                "status": "pending"
            }),
        );

        let store = TaskListStore::new(temp.path());
        let task = store
            .create_task(
                DEFAULT_TASK_LIST_ID,
                NewTaskRecord {
                    subject: "Implement task_create".to_string(),
                    description: "Create shared task files.".to_string(),
                    active_form: Some("Implementing task_create".to_string()),
                    metadata: BTreeMap::new(),
                },
            )
            .expect("task should be created");

        assert_eq!(task.id, "3");
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.blocks, Vec::<String>::new());
        assert_eq!(task.blocked_by, Vec::<String>::new());

        let loaded = store
            .get_task(DEFAULT_TASK_LIST_ID, "3")
            .expect("created task should load")
            .expect("created task should exist");
        assert_eq!(loaded.subject, "Implement task_create");
        assert_eq!(
            loaded.active_form.as_deref(),
            Some("Implementing task_create")
        );
    }

    #[test]
    fn update_task_changes_fields_and_metadata() {
        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Create safe task"))
            .expect("task should be created");

        let outcome = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    subject: Some("Update safe task".to_string()),
                    description: Some("Update a shared task.".to_string()),
                    active_form: Some(Some("Updating safe task".to_string())),
                    owner: Some(Some("agent-a".to_string())),
                    status: Some(TaskStatus::InProgress),
                    metadata: BTreeMap::from([
                        ("keep".to_string(), json!("yes")),
                        ("drop".to_string(), serde_json::Value::Null),
                    ]),
                    ..TaskUpdate::default()
                },
            )
            .expect("task should update");

        assert!(outcome.success);
        assert_eq!(outcome.revision, Some(2));
        assert_eq!(
            outcome.status_change.as_ref().map(|change| &change.to),
            Some(&TaskStatus::InProgress)
        );
        let task = store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("task should load")
            .expect("task should exist");
        assert_eq!(task.subject, "Update safe task");
        assert_eq!(task.description, "Update a shared task.");
        assert_eq!(task.active_form.as_deref(), Some("Updating safe task"));
        assert_eq!(task.owner.as_deref(), Some("agent-a"));
        assert_eq!(task.revision, 2);
        assert_eq!(task.metadata["keep"], "yes");
        assert!(!task.metadata.contains_key("drop"));
    }

    #[test]
    fn update_task_rejects_stale_expected_revision() {
        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Keep original subject"))
            .expect("task should be created");
        store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    subject: Some("First update".to_string()),
                    expected_revision: Some(1),
                    ..TaskUpdate::default()
                },
            )
            .expect("first update should work");

        let outcome = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    subject: Some("Stale update".to_string()),
                    expected_revision: Some(1),
                    ..TaskUpdate::default()
                },
            )
            .expect("stale update should return outcome");

        assert!(!outcome.success);
        assert_eq!(outcome.revision, Some(2));
        assert_eq!(outcome.error.as_deref(), Some("Task revision mismatch"));
        let task = store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("task should load")
            .expect("task should exist");
        assert_eq!(task.subject, "First update");
        assert_eq!(task.revision, 2);
    }

    #[cfg(unix)]
    #[test]
    fn update_task_noop_does_not_write_task_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        let store = TaskListStore::new(temp.path());
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("No-op update"))
            .expect("task should be created");

        fs::set_permissions(&list_dir, fs::Permissions::from_mode(0o555))
            .expect("make task list read-only");
        let outcome = store.update_task(
            DEFAULT_TASK_LIST_ID,
            "1",
            TaskUpdate {
                expected_revision: Some(1),
                ..TaskUpdate::default()
            },
        );
        fs::set_permissions(&list_dir, fs::Permissions::from_mode(0o755))
            .expect("restore task list permissions");
        let outcome = outcome.expect("no-op update should not need a file write");

        assert!(outcome.success);
        assert!(outcome.updated_fields.is_empty());
        assert_eq!(outcome.revision, Some(1));
    }

    #[test]
    fn update_task_claim_owner_rejects_conflicting_claim() {
        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Claim task"))
            .expect("task should be created");

        let claimed = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    claim_owner: Some("agent-a".to_string()),
                    expected_revision: Some(1),
                    ..TaskUpdate::default()
                },
            )
            .expect("claim should work");
        let conflict = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    claim_owner: Some("agent-b".to_string()),
                    expected_revision: claimed.revision,
                    ..TaskUpdate::default()
                },
            )
            .expect("conflict should return outcome");

        assert!(!conflict.success);
        assert_eq!(
            conflict.error.as_deref(),
            Some("Task already owned by agent-a")
        );
        let task = store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("task should load")
            .expect("task should exist");
        assert_eq!(task.owner.as_deref(), Some("agent-a"));
    }

    #[test]
    fn update_task_release_owner_requires_matching_owner() {
        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Release task"))
            .expect("task should be created");
        let claimed = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    claim_owner: Some("agent-a".to_string()),
                    ..TaskUpdate::default()
                },
            )
            .expect("claim should work");

        let wrong_release = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    release_owner: Some("agent-b".to_string()),
                    expected_revision: claimed.revision,
                    ..TaskUpdate::default()
                },
            )
            .expect("wrong release should return outcome");
        assert!(!wrong_release.success);
        assert_eq!(
            wrong_release.error.as_deref(),
            Some("Task is owned by agent-a, not agent-b")
        );

        let released = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    release_owner: Some("agent-a".to_string()),
                    expected_revision: claimed.revision,
                    ..TaskUpdate::default()
                },
            )
            .expect("release should work");
        assert!(released.success);
        let task = store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("task should load")
            .expect("task should exist");
        assert!(task.owner.is_none());
    }

    #[test]
    fn update_task_adds_bidirectional_dependencies() {
        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Prepare dependency"))
            .expect("first task should be created");
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Use dependency"))
            .expect("second task should be created");

        let outcome = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "2",
                TaskUpdate {
                    add_blocked_by: vec!["1".to_string()],
                    ..TaskUpdate::default()
                },
            )
            .expect("dependency should update");

        assert_eq!(outcome.updated_fields, vec!["blockedBy"]);
        let blocker = store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("blocker should load")
            .expect("blocker should exist");
        let blocked = store
            .get_task(DEFAULT_TASK_LIST_ID, "2")
            .expect("blocked should load")
            .expect("blocked should exist");
        assert_eq!(blocker.blocks, vec!["2"]);
        assert_eq!(blocked.blocked_by, vec!["1"]);
    }

    #[test]
    fn update_task_delete_removes_task_and_dependency_references() {
        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Prepare dependency"))
            .expect("first task should be created");
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Use dependency"))
            .expect("second task should be created");
        store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "2",
                TaskUpdate {
                    add_blocked_by: vec!["1".to_string()],
                    ..TaskUpdate::default()
                },
            )
            .expect("dependency should update");

        let outcome = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    delete: true,
                    ..TaskUpdate::default()
                },
            )
            .expect("task should delete");

        assert!(outcome.success);
        assert_eq!(outcome.updated_fields, vec!["deleted"]);
        assert!(
            store
                .get_task(DEFAULT_TASK_LIST_ID, "1")
                .expect("deleted task read should be valid")
                .is_none()
        );
        let remaining = store
            .get_task(DEFAULT_TASK_LIST_ID, "2")
            .expect("remaining task should load")
            .expect("remaining task should exist");
        assert!(remaining.blocked_by.is_empty());
    }

    #[test]
    fn update_task_missing_dependency_does_not_write_field_updates() {
        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());
        store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Keep original subject"))
            .expect("task should be created");

        let err = store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    subject: Some("Do not persist this".to_string()),
                    add_blocks: vec!["2".to_string()],
                    ..TaskUpdate::default()
                },
            )
            .expect_err("missing dependency should fail");

        assert!(err.to_string().contains("blocked task '2' not found"));
        let task = store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("task should load")
            .expect("task should exist");
        assert_eq!(task.subject, "Keep original subject");
        assert!(task.blocks.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn create_task_rejects_symlinked_task_list_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).expect("outside dir");
        symlink(&outside, temp.path().join(DEFAULT_TASK_LIST_ID)).expect("symlink task list");

        let err = store
            .create_task(DEFAULT_TASK_LIST_ID, new_task("Create safe task"))
            .expect_err("symlinked task list directory should fail");

        assert!(err.to_string().contains("is not a real directory"));
    }

    #[test]
    fn list_entries_filter_completed_blockers() {
        let tasks = vec![
            task("1", TaskStatus::Completed, vec![]),
            task("2", TaskStatus::Pending, vec!["1", "3"]),
            task("3", TaskStatus::Pending, vec![]),
        ];

        let entries = task_list_entries(&tasks);

        assert_eq!(entries[1].blocked_by, vec!["3"]);
    }

    #[test]
    fn get_task_reads_camel_case_aliases() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "42",
            json!({
                "id": "42",
                "subject": "Implement shared task read tools",
                "description": "Expose TaskList and TaskGet-compatible output.",
                "activeForm": "Implementing shared task read tools",
                "status": "pending",
                "blocks": ["43"],
                "blockedBy": ["41"]
            }),
        );

        let store = TaskListStore::new(temp.path());
        let task = store
            .get_task(DEFAULT_TASK_LIST_ID, "42")
            .expect("task should load")
            .expect("task should exist");

        assert_eq!(
            task.active_form.as_deref(),
            Some("Implementing shared task read tools")
        );
        assert_eq!(task.blocked_by, vec!["41"]);
    }

    #[test]
    fn task_list_id_is_sanitized() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join("team-alpha");
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "1",
            json!({
                "id": "1",
                "subject": "Sanitized task list",
                "status": "pending"
            }),
        );

        let store = TaskListStore::new(temp.path());
        let tasks = store
            .list_tasks("team/alpha")
            .expect("sanitized list should load");

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "1");
    }

    #[test]
    fn list_tasks_returns_empty_when_list_path_is_not_directory() {
        let temp = tempdir().expect("tempdir");
        fs::write(temp.path().join(DEFAULT_TASK_LIST_ID), b"not a directory").expect("write file");

        let store = TaskListStore::new(temp.path());
        let tasks = store
            .list_tasks(DEFAULT_TASK_LIST_ID)
            .expect("file path should be treated as no task list");

        assert!(tasks.is_empty());
    }

    #[test]
    fn get_task_rejects_path_like_ids() {
        let temp = tempdir().expect("tempdir");
        let store = TaskListStore::new(temp.path());

        for task_id in ["../secret", "nested/task", "nested\\task", "/tmp/task"] {
            let task = store
                .get_task(DEFAULT_TASK_LIST_ID, task_id)
                .expect("invalid task id should not hit the filesystem");
            assert!(
                task.is_none(),
                "task id {task_id:?} should be rejected by the store"
            );
        }
    }

    #[test]
    fn list_tasks_skips_task_files_with_mismatched_ids() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "1",
            json!({
                "id": "2",
                "subject": "Mismatched task id",
                "status": "pending"
            }),
        );

        let store = TaskListStore::new(temp.path());
        let tasks = store
            .list_tasks(DEFAULT_TASK_LIST_ID)
            .expect("mismatched ids should be skipped");

        assert!(tasks.is_empty());
    }

    #[test]
    fn get_task_rejects_task_file_with_mismatched_id() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "1",
            json!({
                "id": "2",
                "subject": "Mismatched task id",
                "status": "pending"
            }),
        );

        let store = TaskListStore::new(temp.path());
        let err = store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect_err("mismatched task id should fail direct reads");

        assert!(err.to_string().contains("does not match file id"));
    }

    #[cfg(unix)]
    #[test]
    fn list_tasks_does_not_follow_symlinked_task_list_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).expect("outside dir");
        write_task(
            &outside,
            "1",
            json!({
                "id": "1",
                "subject": "Outside task",
                "status": "pending"
            }),
        );
        symlink(&outside, temp.path().join(DEFAULT_TASK_LIST_ID)).expect("symlink task list");

        let store = TaskListStore::new(temp.path());
        let tasks = store
            .list_tasks(DEFAULT_TASK_LIST_ID)
            .expect("symlinked list dir should be ignored");

        assert!(tasks.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn task_reads_do_not_follow_symlinked_task_files() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        let outside = temp.path().join("outside");
        fs::create_dir_all(&list_dir).expect("task list dir");
        fs::create_dir_all(&outside).expect("outside dir");
        write_task(
            &outside,
            "1",
            json!({
                "id": "1",
                "subject": "Outside task",
                "status": "pending"
            }),
        );
        symlink(outside.join("1.json"), list_dir.join("1.json")).expect("symlink task file");

        let store = TaskListStore::new(temp.path());
        let tasks = store
            .list_tasks(DEFAULT_TASK_LIST_ID)
            .expect("symlinked task file should be skipped");
        let task = store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("symlinked task file should not be read");

        assert!(tasks.is_empty());
        assert!(task.is_none());
    }

    fn write_task(dir: &Path, id: &str, task: serde_json::Value) {
        fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec_pretty(&task).expect("serialize task"),
        )
        .expect("write task");
    }

    fn new_task(subject: &str) -> NewTaskRecord {
        NewTaskRecord {
            subject: subject.to_string(),
            description: "Create a shared task.".to_string(),
            active_form: None,
            metadata: BTreeMap::new(),
        }
    }

    fn task(id: &str, status: TaskStatus, blocked_by: Vec<&str>) -> TaskRecord {
        TaskRecord {
            id: id.to_string(),
            subject: format!("Task {id}"),
            description: String::new(),
            active_form: None,
            owner: None,
            status,
            revision: 1,
            updated_at: 0,
            blocks: Vec::new(),
            blocked_by: blocked_by.into_iter().map(str::to_string).collect(),
            metadata: BTreeMap::new(),
        }
    }
}
