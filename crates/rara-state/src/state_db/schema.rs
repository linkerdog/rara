use anyhow::Result;
use rusqlite::Connection;

use super::StateDb;

impl StateDb {
    pub(super) fn init_schema(&self) -> Result<()> {
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
                token_budget INTEGER,
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

            CREATE TABLE IF NOT EXISTS goals (
                session_id TEXT PRIMARY KEY,
                objective TEXT NOT NULL,
                condition TEXT,
                status TEXT NOT NULL DEFAULT 'Pursuing',
                token_budget INTEGER,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                turns_completed INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
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
        ensure_column(&conn, "spawn_agent_edges", "token_budget", "INTEGER")?;
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
