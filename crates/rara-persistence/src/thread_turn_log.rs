use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::file_lock::AdvisoryFileLock;
use crate::thread_data::{PersistedTurnEntry, PersistedTurnSummary, turn_preview};

const TURN_LOG_FILE: &str = "turns.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTurnRecord {
    pub summary: PersistedTurnSummary,
    pub entries: Vec<PersistedTurnEntry>,
}

pub fn turn_log_path(root_dir: &Path, session_id: &str) -> PathBuf {
    root_dir.join(session_id).join(TURN_LOG_FILE)
}

pub fn append_turn_record(
    root_dir: &Path,
    session_id: &str,
    ordinal: usize,
    entries: &[PersistedTurnEntry],
) -> Result<PersistedTurnSummary> {
    let path = turn_log_path(root_dir, session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let summary = PersistedTurnSummary {
        ordinal,
        event_count: entries.len(),
        artifact_path: PathBuf::from(session_id)
            .join(TURN_LOG_FILE)
            .display()
            .to_string(),
        preview: turn_preview(entries),
        updated_at: epoch_seconds(),
    };
    let record = PersistedTurnRecord {
        summary: summary.clone(),
        entries: entries.to_vec(),
    };

    let _lock = AdvisoryFileLock::acquire(path.with_extension("lock"))?;
    let mut line = serde_json::to_vec(&record)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open thread turn log {}", path.display()))?;
    file.write_all(&line)?;
    file.sync_data()?;
    if let Some(parent) = path.parent() {
        sync_parent_dir_best_effort(parent);
    }
    Ok(summary)
}

pub fn load_turn_records(root_dir: &Path, session_id: &str) -> Result<Vec<PersistedTurnRecord>> {
    let path = turn_log_path(root_dir, session_id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path)
        .with_context(|| format!("open thread turn log {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut latest_by_ordinal = BTreeMap::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<PersistedTurnRecord>(&line) {
            latest_by_ordinal.insert(record.summary.ordinal, record);
        }
    }
    Ok(latest_by_ordinal.into_values().collect())
}

fn epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn sync_parent_dir_best_effort(parent: &Path) {
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir_best_effort(_parent: &Path) {}

const LIVE_LOG_FILE: &str = "live.jsonl";

/// Append one entry to the per-session live log (realtime persistence).
///
/// Uses buffered I/O without fsync (best-effort) — on crash the last few
/// entries may not survive, but the committed turn log covers full turns.
pub fn append_rollout_fragment(
    root_dir: &Path,
    session_id: &str,
    entry: &PersistedTurnEntry,
) -> Result<()> {
    let dir = root_dir.join(session_id);
    fs::create_dir_all(&dir)?;
    let path = dir.join(LIVE_LOG_FILE);
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open live log {}", path.display()))?;
    file.write_all(&line)?;
    Ok(())
}

/// Remove the live log so resume doesn't load a stale partial turn.
pub fn clear_live_log(root_dir: &Path, session_id: &str) {
    let path = root_dir.join(session_id).join(LIVE_LOG_FILE);
    if let Err(e) = fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!(
            "failed to clear live log for session {session_id}: {e} (path: {})",
            path.display()
        );
    }
}

/// Read all entries from the live log, oldest first.
pub fn load_live_entries(root_dir: &Path, session_id: &str) -> Vec<PersistedTurnEntry> {
    let path = root_dir.join(session_id).join(LIVE_LOG_FILE);
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("failed to open live log for {session_id}: {e}");
            return Vec::new();
        }
    };
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for (i, line_result) in reader.lines().enumerate() {
        match line_result {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => match serde_json::from_str::<PersistedTurnEntry>(&line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    eprintln!("parse error at live log line {i} for {session_id}: {e}");
                }
            },
            Err(e) => {
                eprintln!("i/o error at live log line {i} for {session_id}: {e}");
            }
        }
    }
    entries
}
