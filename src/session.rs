use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use rara_persistence::atomic_file;
use rara_state::state_db::PersistedStructuredRolloutEvent;
use rara_state::thread_rollout_log;
use serde::{Deserialize, Serialize};

use crate::agent::Message;
use crate::session_context::{self, SessionContextSearchHit};
use crate::session_transcript;
use crate::todo::TodoState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedCompactionEvent {
    pub event_index: usize,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub boundary_version: u32,
    #[serde(default)]
    pub replaced_start: Option<usize>,
    #[serde(default)]
    pub replaced_end: Option<usize>,
    #[serde(default)]
    pub metadata_owner: Option<String>,
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
    Transcript,
    SnapshotBackfilled,
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
        session_transcript::sync_history_checkpoint(&self.storage_dir, session_id, history)
            .context("sync session transcript checkpoint")?;
        self.save_history_snapshot(session_id, history)?;
        Ok(())
    }

    pub fn save_history_snapshot(&self, session_id: &str, history: &[Message]) -> Result<()> {
        let path = self.session_history_path(session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string(history)?;
        let tmp_path = path.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        if let Err(err) = atomic_file::replace_file(&tmp_path, &path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        if let Some(parent) = path.parent() {
            sync_parent_dir_best_effort(parent);
        }
        Ok(())
    }

    pub fn thread_transcript_recorder(
        &self,
        session_id: &str,
    ) -> session_transcript::ThreadTranscriptRecorder {
        session_transcript::ThreadTranscriptRecorder::main(&self.storage_dir, session_id)
    }

    pub fn save_session_context_checkpoint(
        &self,
        session_id: &str,
        turn_index: u32,
        text: String,
        vector: Vec<f32>,
    ) -> Result<()> {
        session_context::append_context_checkpoint(
            &self.storage_dir,
            session_id,
            turn_index,
            text,
            vector,
        )
    }

    pub fn search_session_context(
        &self,
        query: &str,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<SessionContextSearchHit>> {
        session_context::search_context_shards(&self.storage_dir, query, query_vector, limit)
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
        if let Err(err) = atomic_file::replace_file(&tmp_path, &path) {
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
        if let Err(err) = atomic_file::replace_file(&tmp_path, &path) {
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

    pub fn load_thread_history(&self, thread_id: &str) -> Result<Vec<Message>> {
        Ok(self.load_thread_history_migration(thread_id)?.history)
    }

    pub fn load_thread_history_migration(
        &self,
        thread_id: &str,
    ) -> Result<PersistedThreadHistoryMigration> {
        let transcript_path =
            session_transcript::main_transcript_path(&self.storage_dir, thread_id);
        if transcript_path.exists() {
            match session_transcript::load_transcript(&transcript_path) {
                Ok(load) => {
                    let history = session_transcript::model_visible_messages(&load.entries);
                    let has_snapshot_fallback = self.session_history_path(thread_id).exists()
                        || self.legacy_session_history_path(thread_id).exists();
                    let should_use_fallback = has_snapshot_fallback
                        && (history.is_empty()
                            || load.parse_errors > 0
                            || transcript_is_shorter_than_snapshot_prefix(
                                self, thread_id, &history,
                            )
                            .unwrap_or(false));
                    if !should_use_fallback && (load.parse_errors == 0 || !has_snapshot_fallback) {
                        return Ok(PersistedThreadHistoryMigration {
                            history,
                            source: PersistedThreadHistorySource::Transcript,
                        });
                    }
                }
                Err(err)
                    if !self.session_history_path(thread_id).exists()
                        && !self.legacy_session_history_path(thread_id).exists() =>
                {
                    return Err(err).context("load canonical session transcript");
                }
                Err(err) => {
                    eprintln!(
                        "Warning: could not load canonical session transcript for {thread_id}: {err}"
                    );
                }
            }
        }

        let path = self.session_history_path(thread_id);
        let (history, source) = if path.exists() {
            let content = fs::read_to_string(path)?;
            let history: Vec<Message> = serde_json::from_str(&content)?;
            let _ =
                session_transcript::write_history_snapshot(&self.storage_dir, thread_id, &history);
            (history, PersistedThreadHistorySource::SnapshotBackfilled)
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
                replaced_start: event.replaced_start,
                replaced_end: event.replaced_end,
                metadata_owner: event.metadata_owner.clone(),
                recent_files: event.recent_files.clone(),
                summary: event.summary.clone(),
            },
        )?;
        Ok(())
    }

    pub fn save_spawn_agent_event(
        &self,
        session_id: &str,
        event_id: &str,
        agent_id: &str,
        name: Option<&str>,
        child_session_id: &str,
        status: &str,
        summary: Option<&str>,
    ) -> Result<()> {
        self.append_rollout_event(
            session_id,
            PersistedStructuredRolloutEvent::SpawnAgent {
                recorded_at: Some(epoch_seconds()),
                event_id: event_id.to_string(),
                agent_id: agent_id.to_string(),
                name: name.map(str::to_string),
                child_session_id: child_session_id.to_string(),
                status: status.to_string(),
                summary: summary.map(str::to_string),
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
                    replaced_start,
                    replaced_end,
                    metadata_owner,
                    recent_files,
                    summary,
                } => Some(PersistedCompactionEvent {
                    event_index,
                    before_tokens,
                    after_tokens,
                    boundary_version,
                    replaced_start,
                    replaced_end,
                    metadata_owner,
                    recent_files,
                    summary,
                }),
                PersistedStructuredRolloutEvent::RuntimeState { .. }
                | PersistedStructuredRolloutEvent::PlanState { .. }
                | PersistedStructuredRolloutEvent::Interaction { .. }
                | PersistedStructuredRolloutEvent::SpawnAgent { .. } => None,
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
        let temp_path = path.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
        fs::write(&temp_path, serde_json::to_string(history)?)?;
        if let Err(err) = atomic_file::replace_file(&temp_path, &path) {
            let _ = fs::remove_file(&temp_path);
            return Err(err);
        }
        // Legacy restore should remain available even if the additive transcript mirror fails.
        let _ = session_transcript::write_history_snapshot(&self.storage_dir, thread_id, history);
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
                    replaced_start: compaction.replaced_start,
                    replaced_end: compaction.replaced_end,
                    metadata_owner: compaction.metadata_owner.clone(),
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

#[cfg(unix)]
fn sync_parent_dir_best_effort(parent: &std::path::Path) {
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir_best_effort(_parent: &std::path::Path) {}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn transcript_is_shorter_than_snapshot_prefix(
    session_manager: &SessionManager,
    thread_id: &str,
    transcript_history: &[Message],
) -> Result<bool> {
    let snapshot_path = session_manager.session_history_path(thread_id);
    if !snapshot_path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(snapshot_path)?;
    let snapshot_history: Vec<Message> = serde_json::from_str(&content)?;
    Ok(transcript_history.len() < snapshot_history.len()
        && snapshot_history.starts_with(transcript_history))
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
                replaced_start: Some(0),
                replaced_end: Some(3),
                metadata_owner: Some("runtime.compaction".to_string()),
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
                replaced_start: None,
                replaced_end: None,
                metadata_owner: None,
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
        assert_eq!(events[0].replaced_start, Some(0));
        assert_eq!(events[0].replaced_end, Some(3));
        assert_eq!(
            events[0].metadata_owner.as_deref(),
            Some("runtime.compaction")
        );
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
    fn load_thread_history_prefers_transcript_over_history_snapshot() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let snapshot_history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!("old snapshot history"),
        }];
        let transcript_history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!("new transcript history"),
        }];
        fs::create_dir_all(
            session_manager
                .session_history_path("thread-transcript-canonical")
                .parent()
                .expect("history parent"),
        )?;
        fs::write(
            session_manager.session_history_path("thread-transcript-canonical"),
            serde_json::to_string(&snapshot_history)?,
        )?;
        session_transcript::write_history_snapshot(
            &session_manager.storage_dir,
            "thread-transcript-canonical",
            &transcript_history,
        )?;

        let migration =
            session_manager.load_thread_history_migration("thread-transcript-canonical")?;

        assert_eq!(migration.history, transcript_history);
        assert_eq!(migration.source, PersistedThreadHistorySource::Transcript);
        Ok(())
    }

    #[test]
    fn load_thread_history_falls_back_to_snapshot_when_transcript_has_parse_errors() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let snapshot_history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!("snapshot fallback history"),
        }];
        session_manager.save_session("thread-damaged-transcript", &snapshot_history)?;
        let transcript_path =
            main_transcript_path(&session_manager.storage_dir, "thread-damaged-transcript");
        use std::io::Write as _;
        let mut file = fs::OpenOptions::new().append(true).open(&transcript_path)?;
        writeln!(file, "{{not valid json")?;

        let migration =
            session_manager.load_thread_history_migration("thread-damaged-transcript")?;

        assert_eq!(migration.history, snapshot_history);
        assert_eq!(
            migration.source,
            PersistedThreadHistorySource::SnapshotBackfilled
        );
        let transcript = load_transcript(&transcript_path)?;
        assert_eq!(transcript.parse_errors, 0);
        assert_eq!(
            model_visible_messages(&transcript.entries),
            snapshot_history
        );

        session_manager.save_session("thread-empty-transcript", &snapshot_history)?;
        fs::write(
            main_transcript_path(&session_manager.storage_dir, "thread-empty-transcript"),
            "",
        )?;

        let empty_migration =
            session_manager.load_thread_history_migration("thread-empty-transcript")?;

        assert_eq!(empty_migration.history, snapshot_history);
        assert_eq!(
            empty_migration.source,
            PersistedThreadHistorySource::SnapshotBackfilled
        );
        Ok(())
    }

    #[test]
    fn load_thread_history_falls_back_when_transcript_is_empty_or_shorter_prefix() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let snapshot_history = vec![
            Message {
                role: "user".to_string(),
                content: serde_json::json!("first"),
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::json!("second"),
            },
        ];
        session_manager.save_session("thread-short-transcript", &snapshot_history)?;
        session_transcript::write_history_snapshot(
            &session_manager.storage_dir,
            "thread-short-transcript",
            &snapshot_history[..1],
        )?;

        let migration = session_manager.load_thread_history_migration("thread-short-transcript")?;

        assert_eq!(migration.history, snapshot_history);
        assert_eq!(
            migration.source,
            PersistedThreadHistorySource::SnapshotBackfilled
        );
        let transcript = load_transcript(&main_transcript_path(
            &session_manager.storage_dir,
            "thread-short-transcript",
        ))?;
        assert_eq!(
            model_visible_messages(&transcript.entries),
            snapshot_history
        );
        Ok(())
    }

    #[test]
    fn snapshot_restore_survives_transcript_backfill_failure() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!("snapshot restore should win"),
        }];
        session_manager.save_session("thread-snapshot-transcript-blocked", &history)?;
        fs::remove_file(main_transcript_path(
            &session_manager.storage_dir,
            "thread-snapshot-transcript-blocked",
        ))?;
        fs::create_dir_all(main_transcript_path(
            &session_manager.storage_dir,
            "thread-snapshot-transcript-blocked",
        ))?;

        let migration =
            session_manager.load_thread_history_migration("thread-snapshot-transcript-blocked")?;

        assert_eq!(migration.history, history);
        assert_eq!(
            migration.source,
            PersistedThreadHistorySource::SnapshotBackfilled
        );
        Ok(())
    }

    #[test]
    fn legacy_history_restore_survives_transcript_backfill_failure() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let legacy_path = session_manager.legacy_session_history_path("thread-transcript-blocked");
        let history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!("legacy restore should win"),
        }];
        fs::write(&legacy_path, serde_json::to_string(&history)?)?;
        fs::create_dir_all(main_transcript_path(
            &session_manager.storage_dir,
            "thread-transcript-blocked",
        ))?;

        let migration =
            session_manager.load_thread_history_migration("thread-transcript-blocked")?;

        assert_eq!(migration.history, history);
        assert_eq!(
            migration.source,
            PersistedThreadHistorySource::LegacyBackfilled
        );
        let canonical_history =
            fs::read_to_string(session_manager.session_history_path("thread-transcript-blocked"))?;
        let canonical_messages: Vec<Message> = serde_json::from_str(&canonical_history)?;
        assert_eq!(canonical_messages, history);
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
    fn save_session_does_not_advance_snapshot_when_transcript_write_fails() -> Result<()> {
        let temp = tempdir()?;
        let session_manager = SessionManager::new_for_rara_dir(temp.path().join(".rara"))?;
        let history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!("must stay out of snapshot when transcript fails"),
        }];
        let transcript_path =
            main_transcript_path(&session_manager.storage_dir, "thread-blocked-transcript");
        fs::create_dir_all(&transcript_path)?;

        let err = session_manager
            .save_session("thread-blocked-transcript", &history)
            .expect_err("transcript path should block checkpoint");

        assert!(
            format!("{err:#}").contains("sync session transcript checkpoint"),
            "error should preserve transcript checkpoint context: {err:#}"
        );
        assert!(
            !session_manager
                .session_history_path("thread-blocked-transcript")
                .exists(),
            "history snapshot must not advance ahead of canonical transcript"
        );
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
                    replaced_start: None,
                    replaced_end: None,
                    metadata_owner: None,
                    recent_files: vec!["src/main.rs".to_string()],
                    summary: "first".to_string(),
                },
                PersistedCompactionEvent {
                    event_index: 2,
                    before_tokens: 220,
                    after_tokens: 80,
                    boundary_version: 2,
                    replaced_start: None,
                    replaced_end: None,
                    metadata_owner: None,
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
