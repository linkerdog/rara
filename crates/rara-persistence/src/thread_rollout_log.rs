use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};

use crate::thread_data::PersistedStructuredRolloutEvent;

#[cfg(test)]
mod tests;

fn rollout_log_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn rollout_events_log_path(root_dir: &Path, thread_id: &str) -> PathBuf {
    root_dir.join(thread_id).join("events.jsonl")
}

pub fn rollout_events_snapshot_path(root_dir: &Path, thread_id: &str) -> PathBuf {
    root_dir.join(thread_id).join("events.json")
}

pub struct RolloutEventRecorder {
    path: PathBuf,
}

impl RolloutEventRecorder {
    pub fn new(root_dir: &Path, thread_id: &str) -> Self {
        Self {
            path: rollout_events_log_path(root_dir, thread_id),
        }
    }

    pub fn append_event(&self, event: &PersistedStructuredRolloutEvent) -> Result<()> {
        append_rollout_event_to_path(&self.path, event)
    }

    pub fn flush(&self) -> Result<()> {
        if let Some(parent) = self.path.parent()
            && parent.exists()
        {
            sync_parent_dir_best_effort(parent);
        }
        Ok(())
    }

    pub fn shutdown(self) -> Result<()> {
        self.flush()
    }
}

pub fn append_rollout_event_line(
    root_dir: &Path,
    thread_id: &str,
    event: &PersistedStructuredRolloutEvent,
) -> Result<()> {
    let recorder = RolloutEventRecorder::new(root_dir, thread_id);
    recorder.append_event(event)?;
    recorder.shutdown()
}

fn append_rollout_event_to_path(
    path: &Path,
    event: &PersistedStructuredRolloutEvent,
) -> Result<()> {
    let _guard = rollout_log_write_lock()
        .lock()
        .expect("rollout log write mutex poisoned");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    fs2::FileExt::lock_exclusive(&file)?;
    // Most appends inspect one byte. Only an interrupted write needs a full
    // scan; never concatenate a new event onto an unterminated record.
    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.rewind()?;
            let mut content = Vec::new();
            file.read_to_end(&mut content)?;
            let (_, valid_len) = parse_rollout_log(&content, path)?;
            if valid_len < content.len() {
                let parent = path.parent().context("rollout log has no parent")?;
                let mut backup = tempfile::Builder::new()
                    .prefix("events.jsonl.recovery-")
                    .tempfile_in(parent)?;
                backup.write_all(&content)?;
                backup.as_file().sync_all()?;
                let (_, backup_path) = backup.keep()?;
                #[cfg(unix)]
                fs::File::open(parent)?.sync_all()?;
                file.set_len(valid_len as u64)?;
                log::warn!(
                    "Recovered truncated rollout log {}; original retained at {}",
                    path.display(),
                    backup_path.display()
                );
            } else {
                file.write_all(b"\n")?;
            }
        }
    }
    serde_json::to_writer(&mut file, event)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir_best_effort(parent: &Path) {
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir_best_effort(_parent: &Path) {}

pub fn load_rollout_events(
    root_dir: &Path,
    thread_id: &str,
) -> Result<Vec<PersistedStructuredRolloutEvent>> {
    let mut events = Vec::new();

    let append_only_path = rollout_events_log_path(root_dir, thread_id);
    if append_only_path.exists() {
        let content = {
            let _guard = rollout_log_write_lock()
                .lock()
                .expect("rollout log write mutex poisoned");
            let mut file = fs::File::open(&append_only_path)?;
            fs2::FileExt::lock_shared(&file)?;
            let mut content = Vec::new();
            file.read_to_end(&mut content)?;
            content
        };
        let (append_only_events, _) = parse_rollout_log(&content, &append_only_path)?;
        events.extend(append_only_events);
    }

    let snapshot_path = rollout_events_snapshot_path(root_dir, thread_id);
    if snapshot_path.exists() {
        let content = fs::read_to_string(snapshot_path)?;
        let snapshot_events =
            serde_json::from_str::<Vec<PersistedStructuredRolloutEvent>>(&content)?;
        events.extend(snapshot_events);
    }

    Ok(events)
}

/// Return complete events and the byte boundary safe for a subsequent append.
fn parse_rollout_log(
    content: &[u8],
    path: &Path,
) -> Result<(Vec<PersistedStructuredRolloutEvent>, usize)> {
    let mut events = Vec::new();
    let mut offset = 0;
    for (index, line) in content.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let line_number = index + 1;
        if !line.iter().all(u8::is_ascii_whitespace) {
            match serde_json::from_slice(line) {
                Ok(event) => events.push(event),
                Err(error) if error.is_eof() && !line.ends_with(b"\n") => {
                    log::warn!(
                        "Ignoring truncated final rollout record at {} line {line_number}: {error}",
                        path.display()
                    );
                    return Ok((events, offset));
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "Invalid rollout record at {} line {line_number}",
                            path.display()
                        )
                    });
                }
            }
        }
        offset += line.len();
    }
    Ok((events, offset))
}
