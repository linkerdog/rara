use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use anyhow::{Context, Result};
use fs2::FileExt;

pub struct AdvisoryFileLock {
    file: File,
}

impl AdvisoryFileLock {
    pub fn acquire(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create lock directory {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open lock file {}", path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("lock file {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for AdvisoryFileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
