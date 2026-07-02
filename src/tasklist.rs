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
        if !task_list_dir.exists() {
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
            if !is_task_file(&path) {
                continue;
            }
            match read_task_file(&path) {
                Ok(task) => tasks.push(task),
                Err(err) => log::warn!("Failed to read task file {}: {err}", path.display()),
            }
        }
        sort_tasks_by_id(&mut tasks);
        Ok(tasks)
    }

    pub fn get_task(&self, task_list_id: &str, task_id: &str) -> Result<Option<TaskRecord>> {
        let task_id = task_id.trim();
        if task_id.is_empty() {
            return Ok(None);
        }

        let path = self
            .task_list_dir(task_list_id)
            .join(format!("{task_id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        read_task_file(&path).map(Some)
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

fn read_task_file(path: &Path) -> Result<TaskRecord> {
    let bytes = fs::read(path).with_context(|| format!("read task file {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse task file {}", path.display()))
}

fn is_task_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_none_or(|name| !name.starts_with('.'))
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
