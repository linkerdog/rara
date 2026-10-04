//! Bounded per-user prompt recall, independent of conversation storage.

use std::collections::{HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::redaction::redact_secrets;

pub const MAX_HISTORY_ENTRIES: usize = 200;
pub const MAX_HISTORY_BYTES: usize = 1024 * 1024;
pub const MAX_PROMPT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptHistoryEntry {
    id: String,
    text: String,
}

impl PromptHistoryEntry {
    /// Filter and redact before handing the prompt to an in-memory or disk queue.
    pub fn new(input: &str) -> Option<Self> {
        let text = sanitized_prompt(input)?;
        Some(Self {
            id: uuid::Uuid::new_v4().to_string(),
            text,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

fn sanitized_prompt(input: &str) -> Option<String> {
    if input.starts_with(' ') || input.len() > MAX_PROMPT_BYTES || input.trim().is_empty() {
        return None;
    }
    let text = redact_secrets(input.trim());
    (text.len() <= MAX_PROMPT_BYTES).then_some(text)
}

#[derive(Debug, Default)]
pub struct PromptHistoryLoad {
    pub entries: Vec<PromptHistoryEntry>,
    pub skipped_lines: usize,
    needs_compaction: bool,
    bytes_on_disk: u64,
}

#[derive(Debug, Clone)]
pub struct PromptHistoryStore {
    home: PathBuf,
}

impl PromptHistoryStore {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.home.join("prompt_history.jsonl")
    }

    /// Read a bounded snapshot without waiting for a writer or creating files.
    pub fn load(&self) -> Result<PromptHistoryLoad> {
        let mut file = match File::open(self.path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PromptHistoryLoad::default());
            }
            Err(error) => return Err(error).context("open prompt history"),
        };
        let length = file.metadata()?.len();
        let start = length.saturating_sub(MAX_HISTORY_BYTES as u64);
        // One preceding byte distinguishes a record boundary from a partial line.
        let offset = start.saturating_sub(1);
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(length - offset).read_to_end(&mut bytes)?;
        let mut result = PromptHistoryLoad {
            needs_compaction: start > 0,
            bytes_on_disk: length,
            ..Default::default()
        };
        let bytes = if start > 0 {
            match bytes.iter().position(|&byte| byte == b'\n') {
                Some(index) => &bytes[index + 1..],
                None => &[],
            }
        } else {
            &bytes[..]
        };
        let mut entries = VecDeque::new();
        for line in bytes.split_inclusive(|&byte| byte == b'\n') {
            if !line.ends_with(b"\n") {
                result.needs_compaction = true;
                break;
            }
            // Report aggregate recovery; parse diagnostics may echo raw secrets.
            let mut entry = match serde_json::from_slice::<PromptHistoryEntry>(line) {
                Ok(entry) if uuid::Uuid::parse_str(&entry.id).is_ok() => entry,
                Ok(_) | Err(_) => {
                    result.skipped_lines += 1;
                    result.needs_compaction = true;
                    continue;
                }
            };
            let Some(text) = sanitized_prompt(&entry.text) else {
                result.skipped_lines += 1;
                result.needs_compaction = true;
                continue;
            };
            result.needs_compaction |= entry.text != text;
            entry.text = text;
            entries.push_back(entry);
            if entries.len() > MAX_HISTORY_ENTRIES {
                entries.pop_front();
                result.needs_compaction = true;
            }
        }
        let mut seen = HashSet::new();
        result.entries = entries
            .into_iter()
            .rev()
            .filter(|entry| {
                let unique = seen.insert(entry.id.clone());
                result.needs_compaction |= !unique;
                unique
            })
            .collect();
        result.entries.reverse();
        Ok(result)
    }

    /// Append under a stable lock inode; repair or compact by atomic replacement.
    pub fn append(&self, entry: &PromptHistoryEntry) -> Result<()> {
        fs::create_dir_all(&self.home).context("create prompt history directory")?;
        let _lock = acquire_lock(&self.home.join("prompt_history.lock"))?;
        let loaded = self.load()?;
        let mut entries = loaded.entries;
        let present = entries.iter().any(|existing| existing.id == entry.id);
        if present && !loaded.needs_compaction {
            return Ok(());
        }
        if !present {
            let Some(text) = sanitized_prompt(&entry.text) else {
                anyhow::bail!("prompt history entry does not meet retention policy");
            };
            anyhow::ensure!(
                uuid::Uuid::parse_str(&entry.id).is_ok(),
                "invalid history ID"
            );
            entries.push(PromptHistoryEntry {
                id: entry.id.clone(),
                text,
            });
        }
        let mut lines = entries
            .iter()
            .map(|entry| {
                let mut line = serde_json::to_vec(entry)?;
                line.push(b'\n');
                Ok(line)
            })
            .collect::<Result<VecDeque<Vec<u8>>>>()?;
        let mut bytes: usize = lines.iter().map(Vec::len).sum();
        let mut compact = loaded.needs_compaction;
        if !present && let Some(line) = lines.back() {
            compact |=
                loaded.bytes_on_disk.saturating_add(line.len() as u64) > MAX_HISTORY_BYTES as u64;
        }
        while lines.len() > MAX_HISTORY_ENTRIES || bytes > MAX_HISTORY_BYTES {
            if let Some(line) = lines.pop_front() {
                bytes -= line.len();
                compact = true;
            }
        }
        if compact {
            let mut temporary = tempfile::NamedTempFile::new_in(&self.home)?;
            for line in &lines {
                temporary.write_all(line)?;
            }
            temporary.as_file().sync_all()?;
            temporary
                .persist(self.path())
                .context("replace prompt history")?;
        } else if let Some(line) = lines.back() {
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(self.path()).context("append prompt history")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(line)?;
            file.sync_data()?;
        }
        Ok(())
    }
}

fn acquire_lock(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).context("open prompt history lock")?;
    for _ in 0..10 {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error).context("lock prompt history"),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "prompt history is busy in another process",
    )
    .into())
}

#[cfg(test)]
#[path = "prompt_history_tests.rs"]
mod tests;
