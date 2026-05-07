use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rara_persistence::atomic_file;
use uuid::Uuid;

use crate::state_db::PersistedThreadRecord;

const THREAD_METADATA_FILE: &str = "thread.json";

pub(crate) fn thread_metadata_path(root_dir: &Path, session_id: &str) -> PathBuf {
    root_dir.join(session_id).join(THREAD_METADATA_FILE)
}

pub(crate) fn load_thread_record(
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

pub(crate) fn write_thread_record(root_dir: &Path, record: &PersistedThreadRecord) -> Result<()> {
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
