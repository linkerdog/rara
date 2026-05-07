use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rara_persistence::file_lock::AdvisoryFileLock;
use serde::{Deserialize, Serialize};

use crate::state_db::{PersistedTurnEntry, PersistedTurnSummary, turn_preview};

const TURN_LOG_FILE: &str = "turns.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedTurnRecord {
    pub summary: PersistedTurnSummary,
    pub entries: Vec<PersistedTurnEntry>,
}

pub(crate) fn turn_log_path(root_dir: &Path, session_id: &str) -> PathBuf {
    root_dir.join(session_id).join(TURN_LOG_FILE)
}

pub(crate) fn append_turn_record(
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

pub(crate) fn load_turn_records(
    root_dir: &Path,
    session_id: &str,
) -> Result<Vec<PersistedTurnRecord>> {
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
