use anyhow::Result;
use rusqlite::params;

use super::{
    PersistedRecentThreadRecord, PersistedRecentThreadSummary, PersistedThreadLineage,
    PersistedThreadRecord, RESUMABLE_SESSION_WHERE, StateDb, epoch_seconds,
};

impl StateDb {
    pub fn set_thread_title(&self, session_id: &str, title: &str) -> Result<()> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let updated = conn.execute(
            "UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?",
            params![title, epoch_seconds(), session_id],
        )?;
        anyhow::ensure!(updated == 1, "thread {session_id} is not indexed");
        Ok(())
    }

    pub fn load_thread_record(&self, session_id: &str) -> Result<Option<PersistedThreadRecord>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let record = conn.query_row(
            "SELECT id, cwd, branch, provider, model, base_url, agent_mode, bash_approval,
                    origin_kind, forked_from_thread_id, created_at, plan_explanation,
                    history_len, transcript_len, updated_at, title
             FROM sessions
             WHERE id = ?",
            params![session_id],
            |row| {
                Ok(PersistedThreadRecord {
                    session_id: row.get(0)?,
                    title: row.get(15)?,
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
                    ), '') AS preview, s.title
             FROM sessions s
             WHERE {RESUMABLE_SESSION_WHERE}
             ORDER BY s.updated_at DESC
             LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(PersistedRecentThreadRecord {
                session_id: row.get(0)?,
                title: row.get(20)?,
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
}
