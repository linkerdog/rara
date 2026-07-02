use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

    fn task_list_dir(&self, task_list_id: &str) -> PathBuf {
        self.root.join(normalize_task_list_id(task_list_id))
    }
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskListEntry {
    pub id: String,
    pub subject: String,
    pub status: TaskStatus,
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
    let task: TaskRecord = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse task file {}", path.display()))?;
    anyhow::ensure!(
        is_valid_task_id(&task.id) && task.id == expected_task_id,
        "task id '{}' does not match file id '{}'",
        task.id,
        expected_task_id
    );
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

    fn task(id: &str, status: TaskStatus, blocked_by: Vec<&str>) -> TaskRecord {
        TaskRecord {
            id: id.to_string(),
            subject: format!("Task {id}"),
            description: String::new(),
            active_form: None,
            owner: None,
            status,
            blocks: Vec::new(),
            blocked_by: blocked_by.into_iter().map(str::to_string).collect(),
            metadata: BTreeMap::new(),
        }
    }
}
