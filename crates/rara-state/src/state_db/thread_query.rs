use anyhow::{Result, anyhow};
use rusqlite::named_params;

use super::{
    PersistedRecentThreadRecord, PersistedThreadLineage, RESUMABLE_SESSION_WHERE, StateDb,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThreadListSort {
    #[default]
    Updated,
    Created,
}

/// Continuation within one unchanged scope, search, and sort order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadListCursor {
    pub timestamp: i64,
    pub session_id: String,
}

#[derive(Debug)]
pub struct ThreadListQuery<'a> {
    pub search: &'a str,
    pub cwd: Option<&'a str>,
    pub exclude_session_id: Option<&'a str>,
    pub sort: ThreadListSort,
    pub after: Option<&'a ThreadListCursor>,
    pub limit: usize,
}

#[derive(Debug)]
pub struct ThreadListPage {
    pub threads: Vec<PersistedRecentThreadRecord>,
    pub next_cursor: Option<ThreadListCursor>,
}

impl StateDb {
    pub fn list_recent_thread_records(
        &self,
        limit: usize,
    ) -> Result<Vec<PersistedRecentThreadRecord>> {
        Ok(self
            .query_threads(ThreadListQuery {
                search: "",
                cwd: None,
                exclude_session_id: None,
                sort: ThreadListSort::Updated,
                after: None,
                limit,
            })?
            .threads)
    }

    /// Query the complete resumable index before limiting a result page.
    pub fn query_threads(&self, query: ThreadListQuery<'_>) -> Result<ThreadListPage> {
        if query.limit == 0 {
            return Ok(ThreadListPage {
                threads: Vec::new(),
                next_cursor: None,
            });
        }
        let order_column = match query.sort {
            ThreadListSort::Updated => "updated_at",
            ThreadListSort::Created => "created_at",
        };
        let sql = format!(
            "WITH candidates AS (
                SELECT s.id, s.cwd, s.branch, s.provider, s.model, s.base_url,
                       s.agent_mode, s.bash_approval, s.created_at, s.history_len, s.transcript_len,
                       s.updated_at, s.origin_kind, s.forked_from_thread_id,
                       s.compaction_count, s.last_compaction_before_tokens,
                       s.last_compaction_after_tokens, s.last_compaction_recent_file_count,
                       s.last_compaction_boundary_version,
                       COALESCE((SELECT preview FROM turns WHERE session_id = s.id
                                 ORDER BY ordinal DESC LIMIT 1), '') AS preview, s.title
                FROM sessions s
                WHERE ({RESUMABLE_SESSION_WHERE})
                  AND (:cwd IS NULL OR s.cwd = :cwd)
                  AND (:excluded IS NULL OR s.id != :excluded)
            )
            SELECT * FROM candidates
            WHERE (:search = ''
                OR instr(lower(title), lower(:search)) > 0
                OR instr(lower(preview), lower(:search)) > 0
                OR instr(lower(id), lower(:search)) > 0
                OR instr(lower(cwd), lower(:search)) > 0
                OR instr(lower(branch), lower(:search)) > 0
                OR instr(lower(provider), lower(:search)) > 0
                OR instr(lower(model), lower(:search)) > 0
                OR instr(lower(agent_mode), lower(:search)) > 0
                OR instr(lower(bash_approval), lower(:search)) > 0)
              AND (:timestamp IS NULL OR ({order_column}, id) < (:timestamp, :last_id))
            ORDER BY {order_column} DESC, id DESC LIMIT :limit"
        );
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow!("state db mutex poisoned"))?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            named_params! {
                ":cwd": query.cwd,
                ":excluded": query.exclude_session_id,
                ":search": query.search.trim(),
                ":timestamp": query.after.map(|cursor| cursor.timestamp),
                ":last_id": query.after.map(|cursor| cursor.session_id.as_str()),
                ":limit": i64::try_from(query.limit.saturating_add(1))?,
            },
            |row| {
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
                        .map(|v| v as usize),
                    last_compaction_after_tokens: row
                        .get::<_, Option<i64>>(16)?
                        .map(|v| v as usize),
                    last_compaction_recent_file_count: row
                        .get::<_, Option<i64>>(17)?
                        .map(|v| v as usize),
                    last_compaction_boundary_version: row
                        .get::<_, Option<i64>>(18)?
                        .map(|v| v as u32),
                    preview: row.get(19)?,
                })
            },
        )?;
        let mut threads = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = threads.len() > query.limit;
        threads.truncate(query.limit);
        let next_cursor =
            has_more
                .then(|| threads.last())
                .flatten()
                .map(|thread| ThreadListCursor {
                    timestamp: match query.sort {
                        ThreadListSort::Updated => thread.updated_at,
                        ThreadListSort::Created => thread.created_at,
                    },
                    session_id: thread.session_id.clone(),
                });
        Ok(ThreadListPage {
            threads,
            next_cursor,
        })
    }
}

#[cfg(test)]
#[path = "thread_query_tests.rs"]
mod tests;
