//! Memory consolidation scheduler (Claude Code style).
//!
//! Checks whether consolidation is due and provides the session list
//! for a forked subagent to write topics/ and MEMORY.md.
//!
//! Scheduling uses three gates:
//!   1. Time gate: at least min_hours since last consolidation
//!   2. Session gate: at least min_sessions new since last
//!   3. Lock gate: no other consolidation in progress
//!
//! The lock file's mtime serves as `lastConsolidatedAt` — no separate
//! state file is needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Consolidation trigger configuration.
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    /// Minimum hours since the last consolidation.
    pub min_hours_since_last: u64,
    /// Minimum new sessions required since last consolidation.
    pub min_new_sessions: u64,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            min_hours_since_last: 24,
            min_new_sessions: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// The lock file (also serves as `lastConsolidatedAt` marker via mtime).
const LOCK_FILE: &str = ".consolidation.lock";
/// Sub-directory for topic files.
pub const TOPICS_DIR: &str = "topics";
/// The MEMORY.md index file.
pub const INDEX_FILE: &str = "MEMORY.md";

// ---------------------------------------------------------------------------
// Lock
// ---------------------------------------------------------------------------

/// Exclusive advisory lock.  Released on drop.
pub struct ConsolidationLock {
    path: PathBuf,
    _file: fs::File,
}

impl ConsolidationLock {
    /// Acquire the consolidation lock, or return `None` if another
    /// instance holds it.
    pub fn acquire(memory_root: &Path) -> Option<Self> {
        let path = memory_root.join(LOCK_FILE);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .ok()?;
        file.try_lock_exclusive().ok()?;
        Some(Self { path, _file: file })
    }
}

impl Drop for ConsolidationLock {
    fn drop(&mut self) {
        // Touch the file so mtime becomes lastConsolidatedAt.
        if let Ok(f) = fs::OpenOptions::new().write(true).open(&self.path) {
            f.set_len(0).ok();
        }
    }
}

// ---------------------------------------------------------------------------
// Session info
// ---------------------------------------------------------------------------

/// Metadata for one session file that the subagent should read.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Absolute path to the session file.
    pub path: PathBuf,
    /// Last modification time (unix seconds).
    pub mtime_secs: u64,
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Consolidation scheduler.
#[derive(Clone)]
pub struct ConsolidationScheduler {
    memory_root: PathBuf,
    config: ConsolidationConfig,
}

impl ConsolidationScheduler {
    pub fn new(memory_root: PathBuf, config: ConsolidationConfig) -> Self {
        Self {
            memory_root,
            config,
        }
    }

    /// Returns new sessions if consolidation is due, or `None`.
    ///
    /// Does NOT acquire the lock — the caller decides when to do that
    /// (typically after confirming there are sessions to process).
    pub fn check(&self) -> Option<Vec<SessionInfo>> {
        let last = self.read_last_consolidated_at();
        let now = epoch_seconds();

        // Time gate.
        if let Some(ts) = last {
            if now.saturating_sub(ts) < self.config.min_hours_since_last * 3600 {
                return None;
            }
        }

        // Gather sessions.
        let sessions = self.list_sessions_since(last);

        // Session gate.
        if (sessions.len() as u64) < self.config.min_new_sessions {
            return None;
        }

        Some(sessions)
    }

    /// Human-readable consolidation status for TUI display.
    pub fn status(&self) -> String {
        let last = self.read_last_consolidated_at();
        let now = epoch_seconds();
        match last {
            None => "no consolidation has run yet".into(),
            Some(ts) => {
                let secs = now.saturating_sub(ts);
                let ago = if secs < 60 {
                    format!("{}s ago", secs)
                } else if secs < 3600 {
                    format!("{}m ago", secs / 60)
                } else if secs < 86400 {
                    format!("{}h ago", secs / 3600)
                } else {
                    format!("{}d ago", secs / 86400)
                };
                format!("last consolidation: {}", ago)
            }
        }
    }

    /// Acquire the consolidation lock.
    pub fn acquire_lock(&self) -> Option<ConsolidationLock> {
        ConsolidationLock::acquire(&self.memory_root)
    }

    fn read_last_consolidated_at(&self) -> Option<u64> {
        let path = self.memory_root.join(LOCK_FILE);
        fs::metadata(&path)
            .ok()?
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    }

    fn list_sessions_since(&self, since: Option<u64>) -> Vec<SessionInfo> {
        let sessions_dir = self.memory_root.join("sessions");
        let dir = match fs::read_dir(&sessions_dir) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        dir.filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            // Skip dotfiles and agent-sidecar logs.
            if name.starts_with('.') || name.starts_with("agent-") {
                return None;
            }
            let meta = fs::metadata(&path).ok()?;
            let mtime = meta.modified().ok()?;
            let mtime_secs = mtime.duration_since(UNIX_EPOCH).ok()?.as_secs();
            if let Some(since) = since {
                if mtime_secs <= since {
                    return None;
                }
            }
            Some(SessionInfo { path, mtime_secs })
        })
        .collect()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
