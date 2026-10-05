use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
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
        .read(true)
        .open(&path)
        .with_context(|| format!("open thread turn log {}", path.display()))?;
    ensure_line_separator(&mut file)?;
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
    append_rollout_fragments(root_dir, session_id, std::slice::from_ref(entry))
}

/// Append a buffered group of live entries with one open and one write.
pub fn append_rollout_fragments(
    root_dir: &Path,
    session_id: &str,
    entries: &[PersistedTurnEntry],
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let dir = root_dir.join(session_id);
    fs::create_dir_all(&dir)?;
    let path = dir.join(LIVE_LOG_FILE);
    let mut lines = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut lines, entry)?;
        lines.push(b'\n');
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)
        .with_context(|| format!("open live log {}", path.display()))?;
    ensure_line_separator(&mut file)?;
    file.write_all(&lines)?;
    Ok(())
}

fn ensure_line_separator(file: &mut fs::File) -> std::io::Result<()> {
    if file.metadata()?.len() == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0];
    file.read_exact(&mut last)?;
    if last[0] != b'\n' {
        // A prior interrupted write must not swallow a successfully retried
        // record. Preserve the fragment and start the next JSON record cleanly.
        file.write_all(b"\n")?;
    }
    Ok(())
}

/// Replace the live log with the current active-turn entries.
pub fn replace_live_entries(
    root_dir: &Path,
    session_id: &str,
    entries: &[PersistedTurnEntry],
) -> Result<()> {
    let dir = root_dir.join(session_id);
    fs::create_dir_all(&dir)?;
    let path = dir.join(LIVE_LOG_FILE);
    let tmp_path = dir.join("live.jsonl.tmp");
    let mut data = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut data, entry)?;
        data.push(b'\n');
    }
    fs::write(&tmp_path, data).with_context(|| format!("write live log {}", tmp_path.display()))?;
    crate::atomic_file::replace_file(&tmp_path, &path)
        .with_context(|| format!("replace live log {}", path.display()))
}

/// Remove the live log so resume doesn't load a stale partial turn.
pub fn clear_live_log(root_dir: &Path, session_id: &str) -> Result<()> {
    let path = root_dir.join(session_id).join(LIVE_LOG_FILE);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("clear live log {}", path.display())),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_retry_after_a_partial_line_preserves_both_complete_records() {
        let dir = tempfile::tempdir().unwrap();
        let entries = vec![PersistedTurnEntry {
            role: "You".into(),
            message: "retained".into(),
        }];
        append_turn_record(dir.path(), "thread", 0, &entries).unwrap();
        let path = turn_log_path(dir.path(), "thread");
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"{\"summary\":")
            .unwrap();
        append_turn_record(dir.path(), "thread", 1, &entries).unwrap();
        let records = load_turn_records(dir.path(), "thread").unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].summary.ordinal, 0);
        assert_eq!(records[1].summary.ordinal, 1);
        assert_eq!(records[1].entries[0].message, "retained");
    }

    #[test]
    fn live_batch_preserves_a_complete_unterminated_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("thread");
        fs::create_dir_all(&root).unwrap();
        let first = PersistedTurnEntry {
            role: "You".into(),
            message: "first".into(),
        };
        let second = PersistedTurnEntry {
            role: "Agent".into(),
            message: "second".into(),
        };
        fs::write(
            root.join(LIVE_LOG_FILE),
            serde_json::to_vec(&first).unwrap(),
        )
        .unwrap();
        append_rollout_fragments(dir.path(), "thread", &[second]).unwrap();
        let entries = load_live_entries(dir.path(), "thread");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "first");
        assert_eq!(entries[1].message, "second");
    }
}
