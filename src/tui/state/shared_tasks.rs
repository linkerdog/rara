use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::sync::oneshot;

use super::TuiApp;
use crate::context::SharedTaskContextView;
use crate::tasklist::{TaskListStore, canonical_task_list_id};

const SHARED_TASK_POLL_INTERVAL: Duration = Duration::from_millis(500);

type ScanResult = Result<(String, Option<SharedTaskContextView>)>;

#[derive(Default)]
pub(super) struct SharedTaskScan {
    task_list_id: String,
    generation: u64,
    pending: Option<(u64, oneshot::Receiver<ScanResult>)>,
}

impl TuiApp {
    pub fn configure_shared_task_watch(&mut self, task_root: PathBuf, task_list_id: &str) {
        let task_list_id = canonical_task_list_id(task_list_id);
        if self.shared_task_root.as_ref() == Some(&task_root)
            && self.shared_task_scan.task_list_id == task_list_id
        {
            return;
        }
        self.shared_task_root = Some(task_root);
        self.shared_task_scan.task_list_id = task_list_id.clone();
        self.shared_task_scan.generation = self.shared_task_scan.generation.wrapping_add(1);
        self.shared_task_fingerprint = None;
        self.shared_task_last_poll = None;
        self.snapshot.shared_tasks = SharedTaskContextView::from_tasks(task_list_id, Vec::new());
    }

    pub fn refresh_shared_tasks_from_store(&mut self, task_list_id: &str) {
        if let Some(root) = self.shared_task_root.clone() {
            self.configure_shared_task_watch(root, task_list_id);
            self.shared_task_scan.generation = self.shared_task_scan.generation.wrapping_add(1);
            self.shared_task_fingerprint = None;
            self.shared_task_last_poll = None;
        }
    }

    pub fn poll_shared_task_files(&mut self) -> bool {
        let mut changed = false;
        if let Some((generation, receiver)) = self.shared_task_scan.pending.as_mut() {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => Some(Err(anyhow::anyhow!(
                    "shared task scan stopped before completion"
                ))),
            };
            if let Some(result) = result {
                let current = *generation == self.shared_task_scan.generation;
                self.shared_task_scan.pending = None;
                if current {
                    self.shared_task_last_poll = Some(Instant::now());
                    match result {
                        Ok((fingerprint, view)) => {
                            self.shared_task_fingerprint = Some(fingerprint);
                            if let Some(view) = view {
                                self.snapshot.shared_tasks = view;
                                changed = true;
                            }
                        }
                        Err(error) => {
                            let error = format!("{error:#}");
                            if self.snapshot.shared_tasks.error.as_deref() != Some(&error) {
                                log::warn!("Failed to refresh shared tasks: {error}");
                            }
                            self.shared_task_fingerprint = None;
                            self.snapshot.shared_tasks = SharedTaskContextView::from_error(
                                self.shared_task_scan.task_list_id.clone(),
                                error,
                            );
                            changed = true;
                        }
                    }
                }
            }
        }
        if self.shared_task_scan.pending.is_some()
            || self
                .shared_task_last_poll
                .is_some_and(|last| last.elapsed() < SHARED_TASK_POLL_INTERVAL)
        {
            return changed;
        }
        let Some(root) = self.shared_task_root.clone() else {
            return changed;
        };
        let task_list_id = self.shared_task_scan.task_list_id.clone();
        let previous = self.shared_task_fingerprint.clone();
        let (sender, receiver) = oneshot::channel();
        self.shared_task_scan.pending = Some((self.shared_task_scan.generation, receiver));
        tokio::task::spawn_blocking(move || {
            let result = (|| {
                let fingerprint = shared_task_fingerprint(&root, &task_list_id)?;
                let view = if previous.as_ref() == Some(&fingerprint) {
                    None
                } else {
                    let tasks = TaskListStore::new(root).list_tasks(&task_list_id)?;
                    Some(SharedTaskContextView::from_tasks(task_list_id, tasks))
                };
                Ok((fingerprint, view))
            })();
            // The view owner may have exited; no durable mutation depends on delivery.
            if let Err(Err(error)) = sender.send(result) {
                log::warn!("Discarded shared task scan failed: {error:#}");
            }
        });
        changed
    }

    pub(crate) fn shared_tasks_loading(&self) -> bool {
        self.shared_task_root.is_some()
            && self.shared_task_fingerprint.is_none()
            && self.snapshot.shared_tasks.error.is_none()
    }

    pub fn switch_active_shared_task_list(&mut self, task_list_id: &str) -> String {
        let task_list_id = canonical_task_list_id(task_list_id);
        self.refresh_shared_tasks_from_store(&task_list_id);
        self.snapshot.shared_tasks.task_list_id = task_list_id.clone();
        task_list_id
    }

    #[cfg(test)]
    pub(crate) async fn finish_shared_task_scan(&mut self) {
        if self.shared_task_scan.pending.is_none() {
            self.poll_shared_task_files();
        }
        let (generation, receiver) = self.shared_task_scan.pending.take().expect("pending scan");
        let result = tokio::time::timeout(Duration::from_secs(5), receiver)
            .await
            .expect("scan timeout")
            .expect("scan worker");
        let (sender, receiver) = oneshot::channel();
        assert!(sender.send(result).is_ok());
        self.shared_task_scan.pending = Some((generation, receiver));
        self.poll_shared_task_files();
    }
}

fn shared_task_fingerprint(task_root: &Path, task_list_id: &str) -> Result<String> {
    let task_list_dir = task_root.join(canonical_task_list_id(task_list_id));
    let entries = match std::fs::read_dir(&task_list_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("missing".into()),
        Err(error) => return Err(error).context("list shared task directory"),
    };
    let mut parts = Vec::new();
    for entry in entries {
        let entry = entry.context("read shared task directory entry")?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("read shared task metadata"),
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .context("read shared task modification time")?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        parts.push(format!(
            "{}:{}:{}:{}",
            file_name,
            metadata.len(),
            modified.as_secs(),
            modified.subsec_nanos()
        ));
    }
    parts.sort();
    Ok(parts.join("|"))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::config::ConfigManager;
    use crate::tasklist::{DEFAULT_TASK_LIST_ID, NewTaskRecord};

    #[tokio::test]
    async fn polls_shared_task_files_when_active_list_changes() {
        let temp = tempdir().expect("tempdir");
        let task_root = temp.path().join(".rara/tasks");
        let store = TaskListStore::new(&task_root);
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.configure_shared_task_watch(task_root.clone(), DEFAULT_TASK_LIST_ID);
        app.refresh_shared_tasks_from_store(DEFAULT_TASK_LIST_ID);
        app.finish_shared_task_scan().await;
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

        app.finish_shared_task_scan().await;
        assert_eq!(app.snapshot.shared_tasks.total, 1);
        assert_eq!(
            app.snapshot.shared_tasks.items[0].subject,
            "Refresh shared tasks"
        );
    }

    #[tokio::test]
    async fn switches_active_shared_task_list_with_canonical_id() {
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
        app.finish_shared_task_scan().await;

        assert_eq!(active, "team-alpha");
        assert_eq!(app.snapshot.shared_tasks.task_list_id, "team-alpha");
        assert_eq!(app.snapshot.shared_tasks.total, 1);
    }

    #[tokio::test]
    async fn shared_task_poll_ignores_dotfiles() {
        let temp = tempdir().expect("tempdir");
        let task_root = temp.path().join(".rara/tasks");
        let store = TaskListStore::new(&task_root);
        store
            .create_task(
                DEFAULT_TASK_LIST_ID,
                NewTaskRecord {
                    subject: "Track visible tasks".to_string(),
                    description: "Ignore hidden editor files.".to_string(),
                    active_form: None,
                    metadata: Default::default(),
                },
            )
            .expect("create task");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.configure_shared_task_watch(task_root.clone(), DEFAULT_TASK_LIST_ID);
        app.switch_active_shared_task_list(DEFAULT_TASK_LIST_ID);
        app.finish_shared_task_scan().await;

        std::fs::write(
            task_root.join(DEFAULT_TASK_LIST_ID).join(".scratch.json"),
            "{}",
        )
        .expect("write dotfile");
        app.shared_task_last_poll = Some(Instant::now() - SHARED_TASK_POLL_INTERVAL);

        let fingerprint = app.shared_task_fingerprint.clone();
        app.finish_shared_task_scan().await;
        assert_eq!(app.shared_task_fingerprint, fingerprint);
        assert_eq!(app.snapshot.shared_tasks.total, 1);
    }
    #[tokio::test]
    async fn old_list_scan_cannot_replace_a_new_binding() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("tasks");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .unwrap();
        app.configure_shared_task_watch(root.clone(), "old");
        app.poll_shared_task_files();
        app.configure_shared_task_watch(root, "new");
        assert!(app.shared_tasks_loading());
        app.finish_shared_task_scan().await;
        assert_eq!(app.snapshot.shared_tasks.task_list_id, "new");
        assert!(
            app.shared_task_scan.pending.is_some(),
            "the new read starts after the stale one finishes"
        );
        app.finish_shared_task_scan().await;
        assert_eq!(app.snapshot.shared_tasks.task_list_id, "new");
        assert!(!app.shared_tasks_loading());
    }
}
