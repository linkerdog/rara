use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use super::TuiApp;
use crate::context::SharedTaskContextView;
use crate::tasklist::{TaskListStore, canonical_task_list_id};

const SHARED_TASK_POLL_INTERVAL: Duration = Duration::from_millis(500);

impl TuiApp {
    pub fn configure_shared_task_watch(&mut self, task_root: PathBuf, task_list_id: &str) {
        self.shared_task_root = Some(task_root);
        self.shared_task_fingerprint = self.current_shared_task_fingerprint(task_list_id);
        self.shared_task_last_poll = Some(Instant::now());
    }

    pub fn refresh_shared_tasks_from_store(&mut self, task_list_id: &str) {
        let task_list_id = canonical_task_list_id(task_list_id);
        let Some(task_root) = self.shared_task_root.as_ref() else {
            return;
        };
        let store = TaskListStore::new(task_root);
        match store.list_tasks(&task_list_id) {
            Ok(tasks) => {
                self.snapshot.shared_tasks = SharedTaskContextView::from_tasks(task_list_id, tasks);
            }
            Err(err) => {
                log::warn!("Failed to refresh shared task list '{task_list_id}': {err}");
                self.snapshot.shared_tasks =
                    SharedTaskContextView::from_error(task_list_id, err.to_string());
            }
        }
    }

    pub fn poll_shared_task_files(&mut self) -> bool {
        if self.shared_task_root.is_none() {
            return false;
        }
        if self
            .shared_task_last_poll
            .is_some_and(|last_poll| last_poll.elapsed() < SHARED_TASK_POLL_INTERVAL)
        {
            return false;
        }
        self.shared_task_last_poll = Some(Instant::now());
        let task_list_id = self.snapshot.shared_tasks.task_list_id.clone();
        if task_list_id.is_empty() {
            return false;
        }
        let Some(next_fingerprint) = self.current_shared_task_fingerprint(&task_list_id) else {
            return false;
        };
        if self.shared_task_fingerprint.as_deref() == Some(next_fingerprint.as_str()) {
            return false;
        }
        self.shared_task_fingerprint = Some(next_fingerprint);
        self.refresh_shared_tasks_from_store(&task_list_id);
        true
    }

    pub fn switch_active_shared_task_list(&mut self, task_list_id: &str) -> String {
        let task_list_id = canonical_task_list_id(task_list_id);
        self.snapshot.shared_tasks.task_list_id = task_list_id.clone();
        self.refresh_shared_tasks_from_store(&task_list_id);
        self.shared_task_fingerprint = self.current_shared_task_fingerprint(&task_list_id);
        self.shared_task_last_poll = Some(Instant::now());
        task_list_id
    }

    fn current_shared_task_fingerprint(&self, task_list_id: &str) -> Option<String> {
        let task_root = self.shared_task_root.as_ref()?;
        Some(shared_task_fingerprint(task_root, task_list_id))
    }
}

fn shared_task_fingerprint(task_root: &Path, task_list_id: &str) -> String {
    let task_list_dir = task_root.join(canonical_task_list_id(task_list_id));
    let Ok(entries) = std::fs::read_dir(&task_list_dir) else {
        return "missing".to_string();
    };
    let mut parts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
        let modified = modified
            .map(|duration| format!("{}:{}", duration.as_secs(), duration.subsec_nanos()))
            .unwrap_or_else(|| "-".to_string());
        parts.push(format!(
            "{}:{}:{}",
            entry.file_name().to_string_lossy(),
            metadata.len(),
            modified
        ));
    }
    parts.sort();
    parts.join("|")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::config::ConfigManager;
    use crate::tasklist::{DEFAULT_TASK_LIST_ID, NewTaskRecord};

    #[test]
    fn polls_shared_task_files_when_active_list_changes() {
        let temp = tempdir().expect("tempdir");
        let task_root = temp.path().join(".rara/tasks");
        let store = TaskListStore::new(&task_root);
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.configure_shared_task_watch(task_root.clone(), DEFAULT_TASK_LIST_ID);
        app.refresh_shared_tasks_from_store(DEFAULT_TASK_LIST_ID);
        assert_eq!(app.snapshot.shared_tasks.total, 0);

        store
            .create_task(
                DEFAULT_TASK_LIST_ID,
                NewTaskRecord {
                    subject: "Refresh shared tasks".to_string(),
                    description: "Detect cross-process updates.".to_string(),
                    active_form: None,
                    metadata: Default::default(),
                },
            )
            .expect("create task");
        app.shared_task_last_poll = Some(Instant::now() - SHARED_TASK_POLL_INTERVAL);

        assert!(app.poll_shared_task_files());
        assert_eq!(app.snapshot.shared_tasks.total, 1);
        assert_eq!(
            app.snapshot.shared_tasks.items[0].subject,
            "Refresh shared tasks"
        );
    }

    #[test]
    fn switches_active_shared_task_list_with_canonical_id() {
        let temp = tempdir().expect("tempdir");
        let task_root = temp.path().join(".rara/tasks");
        let store = TaskListStore::new(&task_root);
        store
            .create_task(
                "team alpha",
                NewTaskRecord {
                    subject: "Use alternate list".to_string(),
                    description: "Switch the active shared task list.".to_string(),
                    active_form: None,
                    metadata: Default::default(),
                },
            )
            .expect("create task");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.configure_shared_task_watch(task_root, DEFAULT_TASK_LIST_ID);

        let active = app.switch_active_shared_task_list("team alpha");

        assert_eq!(active, "team-alpha");
        assert_eq!(app.snapshot.shared_tasks.task_list_id, "team-alpha");
        assert_eq!(app.snapshot.shared_tasks.total, 1);
    }
}
