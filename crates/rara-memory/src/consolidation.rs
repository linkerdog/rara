//! Memory consolidation (dream) runner.
//!
//! Runs a background loop that periodically checks whether consolidation
//! is due and executes the two-phase pipeline:
//!   1. Phase 1 subagents read session files → emit MemoryBatch
//!   2. Phase 2 main model merges batches via an agent turn
//!
//! A PID-based lock file prevents concurrent consolidation runs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(feature = "tokio")]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;

use crate::memory_model::MemoryBatch;

/// Consolidation trigger configuration.
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    /// Minimum hours since the last consolidation.
    pub min_hours_since_last: u64,
    /// Minimum new sessions required.
    pub min_new_sessions: u64,
    /// Scan interval in minutes.
    pub scan_interval_minutes: u64,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            min_hours_since_last: 24,
            min_new_sessions: 5,
            scan_interval_minutes: 10,
        }
    }
}

/// Where the lock file lives (relative to the memory directory).
const LOCK_FILE: &str = ".consolidation.lock";
/// Sub-directory for raw Phase1 outputs.
const RAW_DIR: &str = "raw_memories";
/// Sub-directory for topic files.
const TOPICS_DIR: &str = "topics";
const _INDEX_FILE: &str = "MEMORY.md";

// ---------------------------------------------------------------------------
// Lock
// ---------------------------------------------------------------------------

/// An advisory file lock released on drop.
pub struct ConsolidationLock {
    path: PathBuf,
    _file: fs::File,
}

impl ConsolidationLock {
    pub fn acquire(memory_root: &Path) -> Option<Self> {
        let path = memory_root.join(LOCK_FILE);
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .ok()?;
        file.try_lock_exclusive().ok()?;
        // Write our PID for visibility.
        let _ = file.set_len(0);
        let _ = write!(file, "{}", std::process::id());
        Some(Self { path, _file: file })
    }
}

impl Drop for ConsolidationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// Persistent state needed between consolidation scans.
pub struct ConsolidationRunner {
    memory_root: PathBuf,
    last_consolidated_at: Option<u64>, // unix timestamp seconds
    config: ConsolidationConfig,
}

impl ConsolidationRunner {
    pub fn new(memory_root: PathBuf, config: ConsolidationConfig) -> Self {
        Self {
            last_consolidated_at: last_consolidated_at(&memory_root),
            memory_root,
            config,
        }
    }

    /// Run one scan cycle.  Returns `true` if consolidation was performed.
    pub fn scan(&mut self) -> bool {
        let now = epoch_seconds();
        let min_hours = self.config.min_hours_since_last;
        let min_sessions = self.config.min_new_sessions;

        // Check time gate.
        if let Some(last) = self.last_consolidated_at {
            if now.saturating_sub(last) < min_hours * 3600 {
                return false;
            }
        }

        // Check session gate.
        let new_sessions = count_new_sessions(&self.memory_root, self.last_consolidated_at);
        if new_sessions < min_sessions {
            return false;
        }

        // Acquire lock and run.
        let lock = match ConsolidationLock::acquire(&self.memory_root) {
            Some(l) => l,
            None => return false,
        };

        if let Err(err) = self.run_phases() {
            log::warn!("consolidation error for {:?}: {err}", self.memory_root);
        }

        // Record timestamp even on partial success, so we don't retry too
        // aggressively.
        self.last_consolidated_at = Some(now);
        save_last_consolidated_at(&self.memory_root, now);

        drop(lock);
        true
    }

    fn run_phases(&self) -> Result<(), anyhow::Error> {
        // Phase 1: extract memories from recent sessions.
        // (In the full implementation this dispatches subagents and
        // collects their MemoryBatch outputs.  For now we provide the
        // scaffolding and raw-memory directory.)
        let raw_dir = self.memory_root.join(RAW_DIR);
        fs::create_dir_all(&raw_dir)?;

        // TODO: dispatch subagent extraction, write batch to raw_dir.
        // Placeholder: write an empty batch so Phase2 has something to
        // consume during integration testing.
        let placeholder = MemoryBatch {
            producer: "placeholder".into(),
            entries: vec![],
            nothing_new: true,
        };
        let ts = epoch_seconds();
        let p = raw_dir.join(format!("{ts}.json"));
        let mut f = fs::File::create(&p)?;
        serde_json::to_writer(&mut f, &placeholder)?;

        // Phase 2: merge into topics and MEMORY.md.
        let topics_dir = self.memory_root.join(TOPICS_DIR);
        fs::create_dir_all(&topics_dir)?;
        // TODO: load all raw batches, run main-model merge, update topics/
        // and MEMORY.md.

        let _ = topics_dir;
        Ok(())
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

fn last_consolidated_at(memory_root: &Path) -> Option<u64> {
    let p = memory_root.join(".last_consolidated");
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

fn save_last_consolidated_at(memory_root: &Path, ts: u64) {
    let p = memory_root.join(".last_consolidated");
    let _ = fs::write(p, ts.to_string());
}

fn count_new_sessions(memory_root: &Path, since: Option<u64>) -> u64 {
    // Count session directories created after `since`, recursing into
    // subdirectories and skipping internal rara directories.
    let Ok(entries) = fs::read_dir(memory_root) else {
        return 0;
    };
    let threshold = since.map(|s| s as i64).unwrap_or(0);
    let mut count = 0u64;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == RAW_DIR || name_str == TOPICS_DIR || name_str.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        if let Ok(modified) = meta.modified() {
            if let Ok(dur) = modified.duration_since(UNIX_EPOCH) {
                if dur.as_secs() as i64 > threshold {
                    count += 1;
                }
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Start the background consolidation loop.  Spawns a background task
/// (async) that scans at the configured interval.
#[cfg(feature = "tokio")]
pub async fn start_consolidation_loop(memory_root: PathBuf, config: ConsolidationConfig) {
    let interval = Duration::from_secs(config.scan_interval_minutes * 60);
    let mut runner = ConsolidationRunner::new(memory_root.clone(), config);
    loop {
        runner.scan();
        tokio::time::sleep(interval).await;
    }
}
