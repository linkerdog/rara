use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::thread_rollout_log;

#[cfg(test)]
mod tests;

const RESUMABLE_SESSION_WHERE: &str = "s.history_len > 0
                OR s.compaction_count > 0
                OR s.plan_explanation IS NOT NULL
                OR EXISTS (SELECT 1 FROM turns WHERE session_id = s.id)
                OR EXISTS (SELECT 1 FROM plan_steps WHERE session_id = s.id)
                OR EXISTS (SELECT 1 FROM interactions WHERE session_id = s.id)";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTurnEntry {
    pub role: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedPlanStep {
    pub step_index: usize,
    pub status: String,
    pub step: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedInteraction {
    pub kind: String,
    pub status: String,
    pub title: String,
    pub summary: String,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTurnSummary {
    pub ordinal: usize,
    pub event_count: usize,
    pub artifact_path: String,
    pub preview: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedRuntimeRolloutItem {
    PlanState {
        explanation: Option<String>,
        steps: Vec<PersistedPlanStep>,
    },
    Interaction(PersistedInteraction),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedStructuredRolloutEvent {
    Compaction {
        #[serde(default)]
        recorded_at: Option<i64>,
        event_index: usize,
        before_tokens: usize,
        after_tokens: usize,
        boundary_version: u32,
        #[serde(default)]
        replaced_start: Option<usize>,
        #[serde(default)]
        replaced_end: Option<usize>,
        #[serde(default)]
        metadata_owner: Option<String>,
        recent_files: Vec<String>,
        summary: String,
    },
    RuntimeState {
        #[serde(default)]
        recorded_at: Option<i64>,
        explanation: Option<String>,
        steps: Vec<PersistedPlanStep>,
        interactions: Vec<PersistedInteraction>,
    },
    PlanState {
        #[serde(default)]
        recorded_at: Option<i64>,
        explanation: Option<String>,
        steps: Vec<PersistedPlanStep>,
    },
    Interaction {
        #[serde(default)]
        recorded_at: Option<i64>,
        #[serde(flatten)]
        interaction: PersistedInteraction,
    },
    SpawnAgent {
        #[serde(default)]
        recorded_at: Option<i64>,
        event_id: String,
        agent_id: String,
        name: Option<String>,
        child_session_id: String,
        status: String,
        summary: Option<String>,
    },
}

impl PersistedStructuredRolloutEvent {
    pub fn runtime_state_from_items(
        items: &[PersistedStructuredRolloutEvent],
        recorded_at: Option<i64>,
    ) -> Self {
        let mut explanation = None;
        let mut steps = Vec::new();
        let mut interactions = Vec::new();
        for item in items {
            match item {
                PersistedStructuredRolloutEvent::RuntimeState {
                    recorded_at: _,
                    explanation: item_explanation,
                    steps: item_steps,
                    interactions: item_interactions,
                } => {
                    explanation = item_explanation.clone();
                    steps = item_steps.clone();
                    interactions = item_interactions.clone();
                }
                PersistedStructuredRolloutEvent::PlanState {
                    recorded_at: _,
                    explanation: item_explanation,
                    steps: item_steps,
                } => {
                    explanation = item_explanation.clone();
                    steps = item_steps.clone();
                }
                PersistedStructuredRolloutEvent::Interaction {
                    recorded_at: _,
                    interaction,
                } => {
                    interactions.push(interaction.clone());
                }
                PersistedStructuredRolloutEvent::Compaction { .. }
                | PersistedStructuredRolloutEvent::SpawnAgent { .. } => {}
            }
        }

        PersistedStructuredRolloutEvent::RuntimeState {
            recorded_at,
            explanation,
            steps,
            interactions,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PersistedLegacyRolloutMigration {
    pub structured_events: Vec<PersistedStructuredRolloutEvent>,
    pub runtime_rollout: Vec<PersistedRuntimeRolloutItem>,
    pub source: PersistedLegacyRolloutSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PersistedLegacyRolloutSource {
    StructuredLog,
    LegacyBackfilled,
    #[default]
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedThreadLineage {
    pub origin_kind: String,
    pub forked_from_thread_id: Option<String>,
}

impl Default for PersistedThreadLineage {
    fn default() -> Self {
        Self {
            origin_kind: "fresh".to_string(),
            forked_from_thread_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PersistedRecentThreadSummary {
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub branch: String,
    pub updated_at: i64,
    pub preview: String,
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_file_count: Option<usize>,
    pub last_compaction_boundary_version: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct PersistedRecentThreadRecord {
    pub session_id: String,
    pub cwd: String,
    pub branch: String,
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub agent_mode: String,
    pub bash_approval: String,
    pub created_at: i64,
    pub history_len: usize,
    pub transcript_len: usize,
    pub updated_at: i64,
    pub lineage: PersistedThreadLineage,
    pub preview: String,
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_file_count: Option<usize>,
    pub last_compaction_boundary_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedThreadRecord {
    pub session_id: String,
    pub cwd: String,
    pub branch: String,
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub agent_mode: String,
    pub bash_approval: String,
    pub created_at: i64,
    pub lineage: PersistedThreadLineage,
    pub plan_explanation: Option<String>,
    pub history_len: usize,
    pub transcript_len: usize,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSpawnAgentEdge {
    pub parent_session_id: String,
    pub event_id: String,
    pub agent_id: String,
    pub name: Option<String>,
    pub child_session_id: String,
    pub status: String,
    pub summary: Option<String>,
    pub recorded_at: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct PersistedCompactState {
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_file_count: Option<usize>,
    pub last_compaction_boundary_version: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedPromptRuntimeState {
    pub append_system_prompt: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersistedSessionRuntimeState {
    pub agent_mode: String,
    pub bash_approval: String,
    pub prompt_runtime: PersistedPromptRuntimeState,
}

pub struct StateDb {
    root_dir: PathBuf,
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl StateDb {
    pub fn new() -> Result<Self> {
        let root = std::env::current_dir()?;
        let root_dir = rara_config::workspace_data_dir_for(&root)?;
        Self::new_for_root_dir(root_dir)
    }

    pub fn new_for_root_dir(root_dir: PathBuf) -> Result<Self> {
        if !root_dir.exists() {
            fs::create_dir_all(&root_dir)?;
        }
        let rollout_dir = root_dir.join("rollouts");
        if !rollout_dir.exists() {
            fs::create_dir_all(&rollout_dir)?;
        }
        let path = root_dir.join("state.sqlite3");
        let conn = Connection::open(&path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let db = Self {
            root_dir,
            path,
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn rollout_root(&self) -> PathBuf {
        self.root_dir.join("rollouts")
    }

    pub fn upsert_session(
        &self,
        session_id: &str,
        cwd: &str,
        branch: &str,
        provider: &str,
        model: &str,
        base_url: Option<&str>,
        agent_mode: &str,
        bash_approval: &str,
        plan_explanation: Option<&str>,
        prompt_runtime: &PersistedPromptRuntimeState,
        history_len: usize,
        transcript_len: usize,
        compact_state: &PersistedCompactState,
    ) -> Result<()> {
        self.upsert_session_with_lineage(
            session_id,
            cwd,
            branch,
            provider,
            model,
            base_url,
            agent_mode,
            bash_approval,
            &PersistedThreadLineage::default(),
            plan_explanation,
            prompt_runtime,
            history_len,
            transcript_len,
            compact_state,
        )
    }

    pub fn upsert_session_with_lineage(
        &self,
        session_id: &str,
        cwd: &str,
        branch: &str,
        provider: &str,
        model: &str,
        base_url: Option<&str>,
        agent_mode: &str,
        bash_approval: &str,
        lineage: &PersistedThreadLineage,
        plan_explanation: Option<&str>,
        prompt_runtime: &PersistedPromptRuntimeState,
        history_len: usize,
        transcript_len: usize,
        compact_state: &PersistedCompactState,
    ) -> Result<()> {
        let now = epoch_seconds();
        let conn = self.conn.lock().expect("state db mutex poisoned");
        conn.execute(
            "INSERT INTO sessions (
                id, cwd, branch, provider, model, base_url, agent_mode, bash_approval,
                origin_kind, forked_from_thread_id,
                plan_explanation, prompt_runtime_json, history_len, transcript_len, compaction_count,
                last_compaction_before_tokens, last_compaction_after_tokens,
                last_compaction_recent_file_count, last_compaction_boundary_version,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                cwd = excluded.cwd,
                branch = excluded.branch,
                provider = excluded.provider,
                model = excluded.model,
                base_url = excluded.base_url,
                agent_mode = excluded.agent_mode,
                bash_approval = excluded.bash_approval,
                origin_kind = excluded.origin_kind,
                forked_from_thread_id = excluded.forked_from_thread_id,
                plan_explanation = excluded.plan_explanation,
                prompt_runtime_json = excluded.prompt_runtime_json,
                history_len = excluded.history_len,
                transcript_len = excluded.transcript_len,
                compaction_count = excluded.compaction_count,
                last_compaction_before_tokens = excluded.last_compaction_before_tokens,
                last_compaction_after_tokens = excluded.last_compaction_after_tokens,
                last_compaction_recent_file_count = excluded.last_compaction_recent_file_count,
                last_compaction_boundary_version = excluded.last_compaction_boundary_version,
                updated_at = excluded.updated_at",
            params![
                session_id,
                cwd,
                branch,
                provider,
                model,
                base_url,
                agent_mode,
                bash_approval,
                lineage.origin_kind,
                lineage.forked_from_thread_id,
                plan_explanation,
                serde_json::to_string(prompt_runtime)?,
                history_len as i64,
                transcript_len as i64,
                compact_state.compaction_count as i64,
                compact_state
                    .last_compaction_before_tokens
                    .map(|value| value as i64),
                compact_state
                    .last_compaction_after_tokens
                    .map(|value| value as i64),
                compact_state
                    .last_compaction_recent_file_count
                    .map(|value| value as i64),
                compact_state
                    .last_compaction_boundary_version
                    .map(|value| value as i64),
                now,
                now
            ],
        )?;
        Ok(())
    }

    pub fn load_session_runtime_state(
        &self,
        session_id: &str,
    ) -> Result<Option<PersistedSessionRuntimeState>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let runtime_state = conn.query_row(
            "SELECT agent_mode, bash_approval, prompt_runtime_json
             FROM sessions
             WHERE id = ?",
            params![session_id],
            |row| {
                let prompt_runtime_json: Option<String> = row.get(2)?;
                let prompt_runtime = prompt_runtime_json
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?
                    .unwrap_or_default();
                Ok(PersistedSessionRuntimeState {
                    agent_mode: row.get(0)?,
                    bash_approval: row.get(1)?,
                    prompt_runtime,
                })
            },
        );
        match runtime_state {
            Ok(runtime_state) => Ok(Some(runtime_state)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub fn replace_plan_steps(&self, session_id: &str, steps: &[PersistedPlanStep]) -> Result<()> {
        let mut conn = self.conn.lock().expect("state db mutex poisoned");
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM plan_steps WHERE session_id = ?",
            params![session_id],
        )?;
        for step in steps {
            tx.execute(
                "INSERT INTO plan_steps (session_id, step_index, status, step, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
                params![
                    session_id,
                    step.step_index as i64,
                    step.status,
                    step.step,
                    epoch_seconds()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn replace_interactions(
        &self,
        session_id: &str,
        interactions: &[PersistedInteraction],
    ) -> Result<()> {
        let mut conn = self.conn.lock().expect("state db mutex poisoned");
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM interactions WHERE session_id = ?",
            params![session_id],
        )?;
        for interaction in interactions {
            tx.execute(
                "INSERT INTO interactions (session_id, kind, status, title, summary, payload_json, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    session_id,
                    interaction.kind,
                    interaction.status,
                    interaction.title,
                    interaction.summary,
                    interaction
                        .payload
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    epoch_seconds()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn persist_turn(
        &self,
        session_id: &str,
        ordinal: usize,
        entries: &[PersistedTurnEntry],
    ) -> Result<PersistedTurnSummary> {
        let artifact_path = self.write_turn_artifact(session_id, ordinal, entries)?;
        let preview = turn_preview(entries);
        let now = epoch_seconds();
        let conn = self.conn.lock().expect("state db mutex poisoned");
        conn.execute(
            "INSERT INTO turns (
                session_id, ordinal, event_count, artifact_path, preview, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(session_id, ordinal) DO UPDATE SET
                event_count = excluded.event_count,
                artifact_path = excluded.artifact_path,
                preview = excluded.preview,
                updated_at = excluded.updated_at",
            params![
                session_id,
                ordinal as i64,
                entries.len() as i64,
                artifact_path,
                preview,
                now,
                now
            ],
        )?;
        Ok(PersistedTurnSummary {
            ordinal,
            event_count: entries.len(),
            artifact_path: self
                .artifact_relative_path(session_id, ordinal)
                .display()
                .to_string(),
            preview: turn_preview(entries),
            updated_at: now,
        })
    }

    pub fn load_turn_entries(
        &self,
        session_id: &str,
        ordinal: usize,
    ) -> Result<Vec<PersistedTurnEntry>> {
        let path = self
            .rollout_root()
            .join(self.artifact_relative_path(session_id, ordinal));
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn load_turn_summaries(&self, session_id: &str) -> Result<Vec<PersistedTurnSummary>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT ordinal, event_count, artifact_path, preview, updated_at
             FROM turns
             WHERE session_id = ?
             ORDER BY ordinal ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(PersistedTurnSummary {
                ordinal: row.get::<_, i64>(0)? as usize,
                event_count: row.get::<_, i64>(1)? as usize,
                artifact_path: row.get(2)?,
                preview: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row?);
        }
        Ok(summaries)
    }

    pub fn load_legacy_runtime_rollout(
        &self,
        session_id: &str,
    ) -> Result<Vec<PersistedRuntimeRolloutItem>> {
        let path = self.legacy_runtime_rollout_path(session_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn load_rollout_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<PersistedStructuredRolloutEvent>> {
        thread_rollout_log::load_rollout_events(&self.rollout_root(), session_id)
    }

    pub fn load_legacy_rollout_migration(
        &self,
        session_id: &str,
    ) -> Result<PersistedLegacyRolloutMigration> {
        let structured_events = self.load_rollout_events(session_id)?;
        let runtime_rollout = self.load_legacy_runtime_rollout(session_id)?;
        let source = if !structured_events.is_empty() {
            PersistedLegacyRolloutSource::StructuredLog
        } else if !runtime_rollout.is_empty() {
            PersistedLegacyRolloutSource::LegacyBackfilled
        } else {
            PersistedLegacyRolloutSource::Empty
        };
        let migration = PersistedLegacyRolloutMigration {
            structured_events,
            runtime_rollout,
            source,
        };
        self.backfill_rollout_log_from_legacy(session_id, &migration)?;
        Ok(migration)
    }

    pub fn append_compaction_rollout_event(
        &self,
        session_id: &str,
        event_index: usize,
        before_tokens: usize,
        after_tokens: usize,
        boundary_version: u32,
        recent_files: &[String],
        summary: &str,
    ) -> Result<()> {
        self.append_compaction_rollout_event_with_metadata(
            session_id,
            event_index,
            before_tokens,
            after_tokens,
            boundary_version,
            None,
            None,
            None,
            recent_files,
            summary,
        )
    }

    pub fn append_compaction_rollout_event_with_metadata(
        &self,
        session_id: &str,
        event_index: usize,
        before_tokens: usize,
        after_tokens: usize,
        boundary_version: u32,
        replaced_start: Option<usize>,
        replaced_end: Option<usize>,
        metadata_owner: Option<&str>,
        recent_files: &[String],
        summary: &str,
    ) -> Result<()> {
        self.append_rollout_event_line(
            session_id,
            &PersistedStructuredRolloutEvent::Compaction {
                recorded_at: Some(epoch_seconds()),
                event_index,
                before_tokens,
                after_tokens,
                boundary_version,
                replaced_start,
                replaced_end,
                metadata_owner: metadata_owner.map(str::to_string),
                recent_files: recent_files.to_vec(),
                summary: summary.to_string(),
            },
        )
    }

    pub fn replace_runtime_rollout_events(
        &self,
        session_id: &str,
        items: &[PersistedStructuredRolloutEvent],
    ) -> Result<()> {
        self.append_rollout_event_line(
            session_id,
            &PersistedStructuredRolloutEvent::runtime_state_from_items(
                items,
                Some(epoch_seconds()),
            ),
        )
    }

    pub fn sync_spawn_agent_edges_from_events(
        &self,
        parent_session_id: &str,
        events: &[PersistedStructuredRolloutEvent],
    ) -> Result<()> {
        let desired_edges = spawn_agent_edges_from_events(parent_session_id, events);
        if desired_edges == self.load_spawn_agent_edges(parent_session_id)? {
            return Ok(());
        }

        let mut conn = self.conn.lock().expect("state db mutex poisoned");
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM spawn_agent_edges WHERE parent_session_id = ?",
            params![parent_session_id],
        )?;
        let now = epoch_seconds();
        for edge in desired_edges {
            tx.execute(
                "INSERT INTO spawn_agent_edges (
                    parent_session_id, event_id, agent_id, name, child_session_id,
                    status, summary, recorded_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(parent_session_id, event_id) DO UPDATE SET
                    agent_id = excluded.agent_id,
                    name = excluded.name,
                    child_session_id = excluded.child_session_id,
                    status = excluded.status,
                    summary = excluded.summary,
                    recorded_at = excluded.recorded_at,
                    updated_at = excluded.updated_at",
                params![
                    edge.parent_session_id,
                    edge.event_id,
                    edge.agent_id,
                    edge.name,
                    edge.child_session_id,
                    edge.status,
                    edge.summary,
                    edge.recorded_at,
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_spawn_agent_edges(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<PersistedSpawnAgentEdge>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT parent_session_id, event_id, agent_id, name, child_session_id,
                    status, summary, recorded_at
             FROM spawn_agent_edges
             WHERE parent_session_id = ?
             ORDER BY COALESCE(recorded_at, 0) ASC, event_id ASC",
        )?;
        let rows = stmt.query_map(params![parent_session_id], |row| {
            Ok(PersistedSpawnAgentEdge {
                parent_session_id: row.get(0)?,
                event_id: row.get(1)?,
                agent_id: row.get(2)?,
                name: row.get(3)?,
                child_session_id: row.get(4)?,
                status: row.get(5)?,
                summary: row.get(6)?,
                recorded_at: row.get(7)?,
            })
        })?;
        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    pub fn load_plan_steps(&self, session_id: &str) -> Result<Vec<PersistedPlanStep>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT step_index, status, step
             FROM plan_steps
             WHERE session_id = ?
             ORDER BY step_index ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(PersistedPlanStep {
                step_index: row.get::<_, i64>(0)? as usize,
                status: row.get(1)?,
                step: row.get(2)?,
            })
        })?;
        let mut steps = Vec::new();
        for row in rows {
            steps.push(row?);
        }
        Ok(steps)
    }

    pub fn load_interactions(&self, session_id: &str) -> Result<Vec<PersistedInteraction>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT kind, status, title, summary, payload_json
             FROM interactions
             WHERE session_id = ?
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            let payload_json: Option<String> = row.get(4)?;
            Ok(PersistedInteraction {
                kind: row.get(0)?,
                status: row.get(1)?,
                title: row.get(2)?,
                summary: row.get(3)?,
                payload: payload_json
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?,
            })
        })?;
        let mut interactions = Vec::new();
        for row in rows {
            interactions.push(row?);
        }
        Ok(interactions)
    }

    pub fn load_session_plan_explanation(&self, session_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let explanation = conn.query_row(
            "SELECT plan_explanation FROM sessions WHERE id = ?",
            params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )?;
        Ok(explanation)
    }

    pub fn load_session_compact_state(&self, session_id: &str) -> Result<PersistedCompactState> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let compact_state = conn.query_row(
            "SELECT compaction_count, last_compaction_before_tokens,
                    last_compaction_after_tokens, last_compaction_recent_file_count,
                    last_compaction_boundary_version
             FROM sessions
             WHERE id = ?",
            params![session_id],
            |row| {
                Ok(PersistedCompactState {
                    compaction_count: row.get::<_, i64>(0)? as usize,
                    last_compaction_before_tokens: row
                        .get::<_, Option<i64>>(1)?
                        .map(|value| value as usize),
                    last_compaction_after_tokens: row
                        .get::<_, Option<i64>>(2)?
                        .map(|value| value as usize),
                    last_compaction_recent_file_count: row
                        .get::<_, Option<i64>>(3)?
                        .map(|value| value as usize),
                    last_compaction_boundary_version: row
                        .get::<_, Option<i64>>(4)?
                        .map(|value| value as u32),
                })
            },
        );
        match compact_state {
            Ok(compact_state) => Ok(compact_state),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(PersistedCompactState::default()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn latest_thread_id(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let sql = format!(
            "SELECT id FROM sessions s
             WHERE {RESUMABLE_SESSION_WHERE}
             ORDER BY updated_at DESC
             LIMIT 1"
        );
        let thread_id = conn.query_row(&sql, [], |row| row.get::<_, String>(0));
        match thread_id {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub fn load_thread_record(&self, session_id: &str) -> Result<Option<PersistedThreadRecord>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let record = conn.query_row(
            "SELECT id, cwd, branch, provider, model, base_url, agent_mode, bash_approval,
                    origin_kind, forked_from_thread_id, created_at, plan_explanation,
                    history_len, transcript_len, updated_at
             FROM sessions
             WHERE id = ?",
            params![session_id],
            |row| {
                Ok(PersistedThreadRecord {
                    session_id: row.get(0)?,
                    cwd: row.get(1)?,
                    branch: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    base_url: row.get(5)?,
                    agent_mode: row.get(6)?,
                    bash_approval: row.get(7)?,
                    lineage: PersistedThreadLineage {
                        origin_kind: row.get(8)?,
                        forked_from_thread_id: row.get(9)?,
                    },
                    created_at: row.get(10)?,
                    plan_explanation: row.get(11)?,
                    history_len: row.get::<_, i64>(12)? as usize,
                    transcript_len: row.get::<_, i64>(13)? as usize,
                    updated_at: row.get(14)?,
                })
            },
        );
        match record {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub fn list_recent_thread_summaries(
        &self,
        limit: usize,
    ) -> Result<Vec<PersistedRecentThreadSummary>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let sql = format!(
            "SELECT s.id, s.provider, s.model, s.branch, s.updated_at,
                    s.compaction_count, s.last_compaction_before_tokens,
                    s.last_compaction_after_tokens, s.last_compaction_recent_file_count,
                    s.last_compaction_boundary_version,
                    COALESCE((
                        SELECT preview FROM turns
                        WHERE session_id = s.id
                        ORDER BY ordinal DESC
                        LIMIT 1
                    ), '') AS preview
             FROM sessions s
             WHERE {RESUMABLE_SESSION_WHERE}
             ORDER BY s.updated_at DESC
             LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(PersistedRecentThreadSummary {
                session_id: row.get(0)?,
                provider: row.get(1)?,
                model: row.get(2)?,
                branch: row.get(3)?,
                updated_at: row.get(4)?,
                preview: row.get(10)?,
                compaction_count: row.get::<_, i64>(5)? as usize,
                last_compaction_before_tokens: row
                    .get::<_, Option<i64>>(6)?
                    .map(|value| value as usize),
                last_compaction_after_tokens: row
                    .get::<_, Option<i64>>(7)?
                    .map(|value| value as usize),
                last_compaction_recent_file_count: row
                    .get::<_, Option<i64>>(8)?
                    .map(|value| value as usize),
                last_compaction_boundary_version: row
                    .get::<_, Option<i64>>(9)?
                    .map(|value| value as u32),
            })
        })?;
        let mut threads = Vec::new();
        for row in rows {
            threads.push(row?);
        }
        Ok(threads)
    }

    pub fn list_recent_thread_records(
        &self,
        limit: usize,
    ) -> Result<Vec<PersistedRecentThreadRecord>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let sql = format!(
            "SELECT s.id, s.cwd, s.branch, s.provider, s.model, s.base_url,
                    s.agent_mode, s.bash_approval, s.created_at, s.history_len, s.transcript_len,
                    s.updated_at, s.origin_kind, s.forked_from_thread_id,
                    s.compaction_count, s.last_compaction_before_tokens,
                    s.last_compaction_after_tokens, s.last_compaction_recent_file_count,
                    s.last_compaction_boundary_version,
                    COALESCE((
                        SELECT preview FROM turns
                        WHERE session_id = s.id
                        ORDER BY ordinal DESC
                        LIMIT 1
                    ), '') AS preview
             FROM sessions s
             WHERE {RESUMABLE_SESSION_WHERE}
             ORDER BY s.updated_at DESC
             LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(PersistedRecentThreadRecord {
                session_id: row.get(0)?,
                cwd: row.get(1)?,
                branch: row.get(2)?,
                provider: row.get(3)?,
                model: row.get(4)?,
                base_url: row.get(5)?,
                agent_mode: row.get(6)?,
                bash_approval: row.get(7)?,
                created_at: row.get(8)?,
                history_len: row.get::<_, i64>(9)? as usize,
                transcript_len: row.get::<_, i64>(10)? as usize,
                updated_at: row.get(11)?,
                lineage: PersistedThreadLineage {
                    origin_kind: row.get(12)?,
                    forked_from_thread_id: row.get(13)?,
                },
                compaction_count: row.get::<_, i64>(14)? as usize,
                last_compaction_before_tokens: row
                    .get::<_, Option<i64>>(15)?
                    .map(|value| value as usize),
                last_compaction_after_tokens: row
                    .get::<_, Option<i64>>(16)?
                    .map(|value| value as usize),
                last_compaction_recent_file_count: row
                    .get::<_, Option<i64>>(17)?
                    .map(|value| value as usize),
                last_compaction_boundary_version: row
                    .get::<_, Option<i64>>(18)?
                    .map(|value| value as u32),
                preview: row.get(19)?,
            })
        })?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    fn write_turn_artifact(
        &self,
        session_id: &str,
        ordinal: usize,
        entries: &[PersistedTurnEntry],
    ) -> Result<String> {
        let relative = self.artifact_relative_path(session_id, ordinal);
        let full_path = self.rollout_root().join(&relative);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(entries)?;
        fs::write(&full_path, content)?;
        Ok(relative.display().to_string())
    }

    fn artifact_relative_path(&self, session_id: &str, ordinal: usize) -> PathBuf {
        PathBuf::from(session_id).join(format!("{ordinal:06}.json"))
    }

    fn legacy_runtime_rollout_path(&self, session_id: &str) -> PathBuf {
        self.rollout_root().join(session_id).join("runtime.json")
    }

    fn backfill_rollout_log_from_legacy(
        &self,
        session_id: &str,
        migration: &PersistedLegacyRolloutMigration,
    ) -> Result<()> {
        let path = thread_rollout_log::rollout_events_log_path(&self.rollout_root(), session_id);
        if path.exists() {
            return Ok(());
        }

        let canonical_events = canonical_rollout_events_for_legacy_migration(migration);
        if canonical_events.is_empty() {
            return Ok(());
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = path.with_extension("jsonl.tmp");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)?;
        for event in canonical_events {
            serde_json::to_writer(&mut file, &event)?;
            use std::io::Write;
            file.write_all(b"\n")?;
        }
        fs::rename(temp_path, path)?;
        Ok(())
    }

    pub(crate) fn append_rollout_event_line(
        &self,
        session_id: &str,
        item: &PersistedStructuredRolloutEvent,
    ) -> Result<()> {
        thread_rollout_log::append_rollout_event_line(&self.rollout_root(), session_id, item)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL,
                branch TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                base_url TEXT,
                agent_mode TEXT NOT NULL,
                bash_approval TEXT NOT NULL,
                origin_kind TEXT NOT NULL DEFAULT 'fresh',
                forked_from_thread_id TEXT,
                plan_explanation TEXT,
                prompt_runtime_json TEXT,
                history_len INTEGER NOT NULL DEFAULT 0,
                transcript_len INTEGER NOT NULL DEFAULT 0,
                compaction_count INTEGER NOT NULL DEFAULT 0,
                last_compaction_before_tokens INTEGER,
                last_compaction_after_tokens INTEGER,
                last_compaction_recent_file_count INTEGER,
                last_compaction_boundary_version INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                event_count INTEGER NOT NULL DEFAULT 0,
                artifact_path TEXT NOT NULL,
                preview TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(session_id, ordinal)
            );

            CREATE TABLE IF NOT EXISTS plan_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                status TEXT NOT NULL,
                step TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS interactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                summary TEXT NOT NULL,
                payload_json TEXT,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS spawn_agent_edges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent_session_id TEXT NOT NULL,
                event_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                name TEXT,
                child_session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT,
                recorded_at INTEGER,
                updated_at INTEGER NOT NULL,
                UNIQUE(parent_session_id, event_id)
            );

            CREATE INDEX IF NOT EXISTS idx_turns_session_ordinal
                ON turns(session_id, ordinal);
            CREATE INDEX IF NOT EXISTS idx_plan_steps_session_step
                ON plan_steps(session_id, step_index);
            CREATE INDEX IF NOT EXISTS idx_interactions_session_kind
                ON interactions(session_id, kind);
            CREATE INDEX IF NOT EXISTS idx_spawn_agent_edges_parent_agent
                ON spawn_agent_edges(parent_session_id, agent_id);
            CREATE INDEX IF NOT EXISTS idx_spawn_agent_edges_child
                ON spawn_agent_edges(child_session_id);
            ",
        )?;
        ensure_column(&conn, "sessions", "plan_explanation", "TEXT")?;
        ensure_column(&conn, "sessions", "prompt_runtime_json", "TEXT")?;
        ensure_column(
            &conn,
            "sessions",
            "origin_kind",
            "TEXT NOT NULL DEFAULT 'fresh'",
        )?;
        ensure_column(&conn, "sessions", "forked_from_thread_id", "TEXT")?;
        ensure_column(
            &conn,
            "sessions",
            "compaction_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &conn,
            "sessions",
            "last_compaction_before_tokens",
            "INTEGER",
        )?;
        ensure_column(&conn, "sessions", "last_compaction_after_tokens", "INTEGER")?;
        ensure_column(
            &conn,
            "sessions",
            "last_compaction_recent_file_count",
            "INTEGER",
        )?;
        ensure_column(
            &conn,
            "sessions",
            "last_compaction_boundary_version",
            "INTEGER",
        )?;
        ensure_column(&conn, "interactions", "payload_json", "TEXT")?;
        Ok(())
    }
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn canonical_rollout_events_for_legacy_migration(
    migration: &PersistedLegacyRolloutMigration,
) -> Vec<PersistedStructuredRolloutEvent> {
    let mut events = migration.structured_events.clone();
    if events
        .iter()
        .any(|event| matches!(event, PersistedStructuredRolloutEvent::RuntimeState { .. }))
    {
        return events;
    }

    let saw_plan_state = events
        .iter()
        .any(|event| matches!(event, PersistedStructuredRolloutEvent::PlanState { .. }));
    let saw_interaction = events
        .iter()
        .any(|event| matches!(event, PersistedStructuredRolloutEvent::Interaction { .. }));

    if !saw_plan_state {
        if let Some((explanation, steps)) = legacy_runtime_plan_state(&migration.runtime_rollout) {
            events.push(PersistedStructuredRolloutEvent::PlanState {
                recorded_at: None,
                explanation,
                steps,
            });
        }
    }
    if !saw_interaction {
        events.extend(
            legacy_runtime_interactions(&migration.runtime_rollout)
                .into_iter()
                .map(|interaction| PersistedStructuredRolloutEvent::Interaction {
                    recorded_at: None,
                    interaction,
                }),
        );
    }

    events
}

fn legacy_runtime_plan_state(
    items: &[PersistedRuntimeRolloutItem],
) -> Option<(Option<String>, Vec<PersistedPlanStep>)> {
    items.iter().find_map(|item| match item {
        PersistedRuntimeRolloutItem::PlanState { explanation, steps } => {
            Some((explanation.clone(), steps.clone()))
        }
        PersistedRuntimeRolloutItem::Interaction(_) => None,
    })
}

fn legacy_runtime_interactions(items: &[PersistedRuntimeRolloutItem]) -> Vec<PersistedInteraction> {
    items
        .iter()
        .filter_map(|item| match item {
            PersistedRuntimeRolloutItem::Interaction(interaction) => Some(interaction.clone()),
            PersistedRuntimeRolloutItem::PlanState { .. } => None,
        })
        .collect()
}

fn spawn_agent_edges_from_events(
    parent_session_id: &str,
    events: &[PersistedStructuredRolloutEvent],
) -> Vec<PersistedSpawnAgentEdge> {
    let mut edges = events
        .iter()
        .filter_map(|event| {
            let PersistedStructuredRolloutEvent::SpawnAgent {
                recorded_at,
                event_id,
                agent_id,
                name,
                child_session_id,
                status,
                summary,
            } = event
            else {
                return None;
            };
            Some(PersistedSpawnAgentEdge {
                parent_session_id: parent_session_id.to_string(),
                event_id: event_id.clone(),
                agent_id: agent_id.clone(),
                name: name.clone(),
                child_session_id: child_session_id.clone(),
                status: status.clone(),
                summary: summary.clone(),
                recorded_at: *recorded_at,
            })
        })
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| {
        left.recorded_at
            .unwrap_or_default()
            .cmp(&right.recorded_at.unwrap_or_default())
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    edges
}

pub(crate) fn turn_preview(entries: &[PersistedTurnEntry]) -> String {
    entries
        .iter()
        .find_map(|entry| {
            let first_line = entry.message.lines().next()?.trim();
            if first_line.is_empty() {
                None
            } else {
                Some(format!("{}: {}", entry.role, first_line))
            }
        })
        .unwrap_or_else(|| "empty turn".to_string())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(());
        }
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}
