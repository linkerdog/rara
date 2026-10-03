use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::{StateDb, epoch_seconds};

impl StateDb {
    /// Persist a session goal so it survives restarts.
    /// An explicit creation timestamp distinguishes replacement from mutation.
    pub fn save_goal(&self, session_id: &str, goal: &serde_json::Value) -> Result<()> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let now = epoch_seconds();
        let existing: Option<i64> = conn
            .query_row(
                "SELECT created_at FROM goals WHERE session_id = ?",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?;
        let created = match goal.get("created_at_epoch_seconds") {
            Some(value) => value.as_i64().ok_or_else(|| {
                anyhow::anyhow!("goal creation timestamp must be a non-negative SQLite integer")
            })?,
            None => existing.unwrap_or(now),
        };
        anyhow::ensure!(created >= 0, "goal creation timestamp must be non-negative");
        conn.execute(
            "INSERT OR REPLACE INTO goals
             (session_id, objective, condition, status, token_budget,
              tokens_used, turns_completed, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session_id,
                goal["objective"].as_str().unwrap_or(""),
                goal["condition"].as_str(),
                goal["status"].as_str().unwrap_or("Pursuing"),
                goal["token_budget"].as_i64(),
                goal["tokens_used"].as_i64().unwrap_or(0),
                goal["turns_completed"].as_i64().unwrap_or(0),
                created,
                now,
            ],
        )?;
        Ok(())
    }

    /// Load a previously saved goal for this session, if any.
    pub fn load_goal(&self, session_id: &str) -> Option<serde_json::Value> {
        match self.try_load_goal(session_id) {
            Ok(goal) => goal,
            Err(error) => {
                log::warn!("Failed to load goal for thread {session_id}: {error}");
                None
            }
        }
    }

    /// Distinguish an absent goal from an unreadable persisted row.
    pub fn try_load_goal(&self, session_id: &str) -> Result<Option<serde_json::Value>> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT objective, condition, status, token_budget,
                        tokens_used, turns_completed, created_at
                 FROM goals WHERE session_id = ?",
        )?;
        let row = stmt
            .query_row(params![session_id], |row| {
                Ok(serde_json::json!({
                    "objective": row.get::<_, String>(0)?,
                    "condition": row.get::<_, Option<String>>(1)?,
                    "status": row.get::<_, String>(2)?,
                    "token_budget": row.get::<_, Option<i64>>(3)?,
                    "tokens_used": row.get::<_, i64>(4)?,
                    "turns_completed": row.get::<_, i64>(5)?,
                    "created_at_epoch_seconds": row.get::<_, i64>(6)?,
                }))
            })
            .optional()?;
        Ok(row)
    }

    /// Clearing a goal must survive a later thread resume.
    pub fn delete_goal(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("state db mutex poisoned");
        conn.execute(
            "DELETE FROM goals WHERE session_id = ?",
            params![session_id],
        )?;
        Ok(())
    }
}
