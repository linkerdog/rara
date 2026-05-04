use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::agent::Message;
use crate::session_transcript;
use crate::state_db::PersistedStructuredRolloutEvent;
use crate::thread_rollout_log;
use crate::todo::TodoState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedCompactionEvent {
    pub event_index: usize,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub boundary_version: u32,
    pub recent_files: Vec<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Default)]
pub struct PersistedThreadHistoryMigration {
    pub history: Vec<Message>,
    pub source: PersistedThreadHistorySource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PersistedThreadHistorySource {
    #[default]
    Canonical,
    LegacyBackfilled,
}

#[derive(Debug, Clone, Default)]
pub struct PersistedCompactionEventsMigration {
    pub events: Vec<PersistedCompactionEvent>,
    pub source: PersistedCompactionEventsSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PersistedCompactionEventsSource {
    StructuredLog,
    LegacyBackfilled,
    #[default]
    Empty,
}

pub struct SessionManager {
    pub storage_dir: PathBuf,
    pub legacy_storage_dir: PathBuf,
}

impl SessionManager {
    pub fn is_missing_thread_history_error(err: &anyhow::Error) -> bool {
        err.to_string().contains("Thread not found locally")
    }

    pub fn new() -> Result<Self> {
        let root = std::env::current_dir()?;
        let rara_dir = rara_config::workspace_data_dir_for(&root)?;
        Self::new_for_rara_dir(rara_dir)
    }

    pub fn new_for_rara_dir(rara_dir: PathBuf) -> Result<Self> {
        let local_dir = rara_dir.join("rollouts");
        let legacy_storage_dir = rara_dir.join("sessions");
        if !local_dir.exists() {
            fs::create_dir_all(&local_dir)?;
        }
        if !legacy_storage_dir.exists() {
            fs::create_dir_all(&legacy_storage_dir)?;
        }
        Ok(Self {
            storage_dir: local_dir,
            legacy_storage_dir,
        })
    }

    pub fn save_session(&self, session_id: &str, history: &[Message]) -> Result<()> {
        let path = self.session_history_path(session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string(history)?;
        let tmp_path = path.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
        fs::write(&tmp_path, content)?;
        if let Err(err) = Self::replace_file(&tmp_path, &path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        session_transcript::write_history_snapshot(&self.storage_dir, session_id, history)
            .context("write session transcript snapshot")?;
        Ok(())
    }

    pub fn plan_file_path(&self, session_id: &str) -> PathBuf {
        self.legacy_storage_dir.join(session_id).join("plan.md")
    }

    pub fn todo_file_path(&self, session_id: &str) -> PathBuf {
        self.legacy_storage_dir.join(session_id).join("todo.json")
    }

    pub fn save_plan_file(&self, session_id: &str, plan: &str) -> Result<()> {
        let path = self.plan_file_path(session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = path.with_extension(format!("md.tmp-{}", uuid::Uuid::new_v4()));
        fs::write(&tmp_path, plan)?;
        if let Err(err) = Self::replace_file(&tmp_path, &path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        Ok(())
    }

    pub fn save_todo_state(&self, session_id: &str, state: &TodoState) -> Result<()> {
        let path = self.todo_file_path(session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(state)?;
        let tmp_path = path.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
        fs::write(&tmp_path, content)?;
        if let Err(err) = Self::replace_file(&tmp_path, &path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        Ok(())
    }

    pub fn load_todo_state(&self, session_id: &str) -> Result<Option<TodoState>> {
        let path = self.todo_file_path(session_id);
        match fs::read_to_string(path) {
            Ok(content) => Ok(Some(serde_json::from_str(&content)?)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    #[cfg(not(windows))]
    fn replace_file(src: &Path, dst: &Path) -> Result<()> {
        fs::rename(src, dst)?;
        Ok(())
    }

    #[cfg(windows)]
    fn replace_file(src: &Path, dst: &Path) -> Result<()> {
        match fs::rename(src, dst) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && dst.exists() => {
                fs::remove_file(dst)?;
                fs::rename(src, dst)?;
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }

    pub fn load_thread_history(&self, thread_id: &str) -> Result<Vec<Message>> {
        Ok(self.load_thread_history_migration(thread_id)?.history)
    }

    pub fn load_thread_history_migration(
        &self,
        thread_id: &str,
    ) -> Result<PersistedThreadHistoryMigration> {
        let path = self.session_history_path(thread_id);
        let (history, source) = if path.exists() {
            let content = fs::read_to_string(path)?;
            (
                serde_json::from_str(&content)?,
                PersistedThreadHistorySource::Canonical,
            )
        } else {
            let legacy = self.legacy_session_history_path(thread_id);
            if !legacy.exists() {
                return Err(anyhow!("Thread not found locally"));
            }
            let content = fs::read_to_string(&legacy)?;
            let history: Vec<Message> = serde_json::from_str(&content)?;
            self.backfill_legacy_thread_history(thread_id, &history)?;
            (history, PersistedThreadHistorySource::LegacyBackfilled)
        };
        Ok(PersistedThreadHistoryMigration { history, source })
    }

    pub fn load_session(&self, session_id: &str) -> Result<Vec<Message>> {
        self.load_thread_history(session_id)
    }

    pub fn save_compaction_event(
        &self,
        session_id: &str,
        event: &PersistedCompactionEvent,
    ) -> Result<()> {
        self.append_rollout_event(
            session_id,
            PersistedStructuredRolloutEvent::Compaction {
                recorded_at: Some(epoch_seconds()),
                event_index: event.event_index,
                before_tokens: event.before_tokens,
                after_tokens: event.after_tokens,
                boundary_version: event.boundary_version,
                recent_files: event.recent_files.clone(),
                summary: event.summary.clone(),
            },
        )?;
        Ok(())
    }

    pub fn load_compaction_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<PersistedCompactionEvent>> {
        Ok(self.load_compaction_events_migration(session_id)?.events)
    }

    pub fn load_compaction_events_migration(
        &self,
        session_id: &str,
    ) -> Result<PersistedCompactionEventsMigration> {
        let events = self.load_structured_rollout_events(session_id)?;
        let structured_compactions = events
            .into_iter()
            .filter_map(|event| match event {
                PersistedStructuredRolloutEvent::Compaction {
                    recorded_at: _,
                    event_index,
                    before_tokens,
                    after_tokens,
                    boundary_version,
                    recent_files,
                    summary,
                } => Some(PersistedCompactionEvent {
                    event_index,
                    before_tokens,
                    after_tokens,
                    boundary_version,
                    recent_files,
                    summary,
                }),
                PersistedStructuredRolloutEvent::RuntimeState { .. }
                | PersistedStructuredRolloutEvent::PlanState { .. }
                | PersistedStructuredRolloutEvent::Interaction { .. } => None,
            })
            .collect::<Vec<_>>();
        if !structured_compactions.is_empty() {
            return Ok(PersistedCompactionEventsMigration {
                events: structured_compactions,
                source: PersistedCompactionEventsSource::StructuredLog,
            });
        }
        let path = self.session_compaction_events_path(session_id);
        if !path.exists() {
            return Ok(PersistedCompactionEventsMigration {
                events: Vec::new(),
                source: PersistedCompactionEventsSource::Empty,
            });
        }
        let content = fs::read_to_string(path)?;
        let compactions: Vec<PersistedCompactionEvent> = serde_json::from_str(&content)?;
        self.backfill_legacy_compaction_events(session_id, &compactions)?;
        Ok(PersistedCompactionEventsMigration {
            events: compactions,
            source: PersistedCompactionEventsSource::LegacyBackfilled,
        })
    }

    pub fn get_context(
        &self,
        session_id: &str,
        turn_index: usize,
        window: usize,
    ) -> Result<Vec<Message>> {
        let history = self.load_thread_history(session_id)?;
        let start = turn_index.saturating_sub(window);
        let end = (turn_index + window + 1).min(history.len());
        Ok(history[start..end].to_vec())
    }

    fn session_history_path(&self, session_id: &str) -> PathBuf {
        self.storage_dir.join(session_id).join("history.json")
    }

    fn legacy_session_history_path(&self, session_id: &str) -> PathBuf {
        self.legacy_storage_dir.join(format!("{}.json", session_id))
    }

    fn session_compaction_events_path(&self, session_id: &str) -> PathBuf {
        self.storage_dir.join(session_id).join("compactions.json")
    }

    fn append_rollout_event(
        &self,
        session_id: &str,
        event: PersistedStructuredRolloutEvent,
    ) -> Result<()> {
        thread_rollout_log::append_rollout_event_line(&self.storage_dir, session_id, &event)
    }

    fn backfill_legacy_thread_history(&self, thread_id: &str, history: &[Message]) -> Result<()> {
        if history.is_empty() {
            return Ok(());
        }
        let path = self.session_history_path(thread_id);
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, serde_json::to_string(history)?)?;
        fs::rename(temp_path, path)?;
        session_transcript::write_history_snapshot(&self.storage_dir, thread_id, history)
            .context("write legacy session transcript snapshot")?;
        Ok(())
    }

    fn backfill_legacy_compaction_events(
        &self,
        session_id: &str,
        compactions: &[PersistedCompactionEvent],
    ) -> Result<()> {
        if compactions.is_empty() {
            return Ok(());
        }
        let rollout_path =
            thread_rollout_log::rollout_events_log_path(&self.storage_dir, session_id);
        let existing_compaction_count = if rollout_path.exists() {
            self.load_structured_rollout_events(session_id)?
                .into_iter()
                .filter(|event| matches!(event, PersistedStructuredRolloutEvent::Compaction { .. }))
                .count()
        } else {
            0
        };
        if existing_compaction_count >= compactions.len() {
            return Ok(());
        }

        for compaction in compactions.iter().skip(existing_compaction_count) {
            self.append_rollout_event(
                session_id,
                PersistedStructuredRolloutEvent::Compaction {
                    recorded_at: None,
                    event_index: compaction.event_index,
                    before_tokens: compaction.before_tokens,
                    after_tokens: compaction.after_tokens,
                    boundary_version: compaction.boundary_version,
                    recent_files: compaction.recent_files.clone(),
                    summary: compaction.summary.clone(),
                },
            )?;
        }
        Ok(())
    }

    fn load_structured_rollout_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<PersistedStructuredRolloutEvent>> {
        thread_rollout_log::load_rollout_events(&self.storage_dir, session_id)
    }
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::session_transcript::{
        SessionTranscriptEntry, load_transcript, main_transcript_path, model_visible_messages,
    };

    #[test]
    fn save_compaction_event_appends_jsonl_lines() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;

        session_manager.save_compaction_event(
            "thread-1",
            &PersistedCompactionEvent {
                event_index: 1,
                before_tokens: 100,
                after_tokens: 40,
                boundary_version: 1,
                recent_files: vec!["src/main.rs".to_string()],
                summary: "first".to_string(),
            },
        )?;
        session_manager.save_compaction_event(
            "thread-1",
            &PersistedCompactionEvent {
                event_index: 2,
                before_tokens: 200,
                after_tokens: 80,
                boundary_version: 2,
                recent_files: vec!["src/thread_store.rs".to_string()],
                summary: "second".to_string(),
            },
        )?;

        let path =
            thread_rollout_log::rollout_events_log_path(&session_manager.storage_dir, "thread-1");
        let content = fs::read_to_string(path)?;
        assert_eq!(content.lines().count(), 2);

        let events = session_manager.load_compaction_events("thread-1")?;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].summary, "first");
        assert_eq!(events[1].summary, "second");
        Ok(())
    }

    #[test]
    fn load_thread_history_backfills_legacy_session_file_into_rollout_root() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let legacy_path = session_manager.legacy_session_history_path("thread-legacy-history");
        fs::write(
            &legacy_path,
            serde_json::to_string(&vec![Message {
                role: "user".to_string(),
                content: serde_json::json!("hello from legacy history"),
            }])?,
        )?;

        let migration = session_manager.load_thread_history_migration("thread-legacy-history")?;
        assert_eq!(migration.history.len(), 1);

        let canonical_history =
            fs::read_to_string(session_manager.session_history_path("thread-legacy-history"))?;
        let canonical_messages: Vec<Message> = serde_json::from_str(&canonical_history)?;
        assert_eq!(canonical_messages.len(), 1);
        assert_eq!(canonical_messages[0].role, "user");
        let transcript = load_transcript(&main_transcript_path(
            &session_manager.storage_dir,
            "thread-legacy-history",
        ))?;
        assert_eq!(
            model_visible_messages(&transcript.entries),
            canonical_messages
        );
        Ok(())
    }

    #[test]
    fn save_session_writes_history_without_leaving_temp_files() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!([{"type": "text", "text": "hello"}]),
        }];

        session_manager.save_session("thread-atomic-history", &history)?;

        let path = session_manager.session_history_path("thread-atomic-history");
        let persisted: Vec<Message> = serde_json::from_str(&fs::read_to_string(&path)?)?;
        assert_eq!(persisted, history);
        let transcript_path =
            main_transcript_path(&session_manager.storage_dir, "thread-atomic-history");
        let transcript = load_transcript(&transcript_path)?;
        assert_eq!(transcript.parse_errors, 0);
        assert_eq!(model_visible_messages(&transcript.entries), history);
        assert!(matches!(
            &transcript.entries[0],
            SessionTranscriptEntry::SessionMeta {
                session_id,
                is_sidechain: false,
                ..
            } if session_id == "thread-atomic-history"
        ));
        let leftovers = fs::read_dir(path.parent().expect("history parent"))?
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);
        Ok(())
    }

    #[test]
    fn save_and_load_todo_state_roundtrips_session_artifact() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let state = crate::todo::TodoState {
            version: 1,
            updated_at: 42,
            items: vec![crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Implement todo runtime".to_string(),
                status: crate::todo::TodoStatus::InProgress,
                updated_at: 42,
            }],
        };

        session_manager.save_todo_state("thread-todo", &state)?;

        assert_eq!(session_manager.load_todo_state("thread-todo")?, Some(state));
        assert!(session_manager.todo_file_path("thread-todo").exists());
        Ok(())
    }

    #[test]
    fn load_todo_state_returns_none_when_missing() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;

        assert_eq!(session_manager.load_todo_state("missing-thread")?, None);
        Ok(())
    }

    #[test]
    fn load_compaction_events_backfills_legacy_compactions_json_into_rollout_log() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let legacy_path = session_manager.session_compaction_events_path("thread-legacy");
        fs::create_dir_all(legacy_path.parent().expect("legacy compaction dir"))?;
        fs::write(
            &legacy_path,
            serde_json::to_string_pretty(&vec![
                PersistedCompactionEvent {
                    event_index: 1,
                    before_tokens: 100,
                    after_tokens: 40,
                    boundary_version: 1,
                    recent_files: vec!["src/main.rs".to_string()],
                    summary: "first".to_string(),
                },
                PersistedCompactionEvent {
                    event_index: 2,
                    before_tokens: 220,
                    after_tokens: 80,
                    boundary_version: 2,
                    recent_files: vec!["src/thread_store.rs".to_string()],
                    summary: "second".to_string(),
                },
            ])?,
        )?;

        let events = session_manager.load_compaction_events("thread-legacy")?;
        assert_eq!(events.len(), 2);

        let rollout_content = fs::read_to_string(thread_rollout_log::rollout_events_log_path(
            &session_manager.storage_dir,
            "thread-legacy",
        ))?;
        let rollout_events = rollout_content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<PersistedStructuredRolloutEvent>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(rollout_events.len(), 2);
        assert!(matches!(
            &rollout_events[0],
            PersistedStructuredRolloutEvent::Compaction {
                event_index,
                summary,
                ..
            } if *event_index == 1 && summary == "first"
        ));
        assert!(matches!(
            &rollout_events[1],
            PersistedStructuredRolloutEvent::Compaction {
                event_index,
                summary,
                ..
            } if *event_index == 2 && summary == "second"
        ));
        Ok(())
    }
}
