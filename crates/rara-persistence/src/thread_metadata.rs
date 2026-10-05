use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use uuid::Uuid;

use crate::atomic_file;
use crate::thread_data::PersistedThreadRecord;

const THREAD_METADATA_FILE: &str = "thread.json";

/// Serialize metadata/index updates across clients. The callback must not acquire
/// the same thread lock again; keep the stable lock inode across file replacement.
pub fn with_thread_record_lock<T>(
    root_dir: &Path,
    session_id: &str,
    update: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let directory = root_dir.join(session_id);
    fs::create_dir_all(&directory)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("metadata.lock"))?;
    for _ in 0..10 {
        match lock.try_lock_exclusive() {
            Ok(()) => return update(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => return Err(error).context("lock thread metadata"),
        }
    }
    anyhow::bail!("thread metadata is busy in another process")
}

pub(crate) fn thread_metadata_path(root_dir: &Path, session_id: &str) -> PathBuf {
    root_dir.join(session_id).join(THREAD_METADATA_FILE)
}

pub fn load_thread_record(
    root_dir: &Path,
    session_id: &str,
) -> Result<Option<PersistedThreadRecord>> {
    let path = thread_metadata_path(root_dir, session_id);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let record = serde_json::from_str(&content)
        .with_context(|| format!("parse thread metadata {}", path.display()))?;
    Ok(Some(record))
}

pub fn write_thread_record(root_dir: &Path, record: &PersistedThreadRecord) -> Result<()> {
    let path = thread_metadata_path(root_dir, &record.session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(record)?;
    let tmp_path = path.with_extension(format!("json.tmp-{}", Uuid::new_v4()));
    {
        let mut file = fs::File::create(&tmp_path)
            .with_context(|| format!("create {}", tmp_path.display()))?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(err) = atomic_file::replace_file(&tmp_path, &path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err).with_context(|| format!("replace thread metadata {}", path.display()));
    }
    if let Some(parent) = path.parent() {
        sync_parent_dir_best_effort(parent);
    }
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
