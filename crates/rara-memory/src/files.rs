//! Durable session and global memory files under `~/.rara/memory/`.
//!
//! Implements `docs/features/session-global-memory.md`:
//! session-scoped `.md` files, global `MEMORY.md`, `memory_summary.md` index,
//! concurrent-safe writes via atomic temp-file + rename, and a unified
//! `search_memory` that returns local text-search results.
//!
//! Concurrent model: every write to a memory file uses atomic
//! temp-file + rename.  Writes that rewrite an entire file (memory_summary.md)
//! additionally acquire a `fs2` exclusive lock so that parallel agents
//! serialise their index updates.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fs2::FileExt;

/// Returns the memory root directory under the given RARA home.
pub fn memory_dir(rara_home: &Path) -> PathBuf {
    rara_home.join("memory")
}

/// Creates the memory directory hierarchy if it doesn't exist.
pub fn ensure_memory_dir(rara_home: &Path) -> Result<PathBuf> {
    let dir = memory_dir(rara_home);
    fs::create_dir_all(dir.join("sessions"))?;
    Ok(dir)
}

/// Returns the path to a session memory file.
pub fn session_memory_path(rara_home: &Path, session_id: &str) -> Result<PathBuf> {
    let dir = ensure_memory_dir(rara_home)?;
    Ok(dir.join("sessions").join(format!("{}.md", session_id)))
}

/// Returns the path to the global memory file.
pub fn global_memory_path(rara_home: &Path) -> Result<PathBuf> {
    let dir = ensure_memory_dir(rara_home)?;
    Ok(dir.join("global.md"))
}

/// Appends content to a memory file.
///
/// Uses atomic append (read-merge-write via temp file + rename) so that
/// concurrent writers never see a partially-written file.
pub fn write_memory(path: &Path, content: &str) -> Result<()> {
    validate_memory_path(path)?;
    // Per-file lock to serialise concurrent writes to the same file
    let lock_path = path.with_extension("lock");
    let lock_file = File::create(&lock_path)
        .with_context(|| format!("create lock file {}", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("acquire lock {}", lock_path.display()))?;
    let existing = if path.exists() {
        fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    let new_content = format!("{}{}\n", existing, content.trim_end());
    atomic_write(path, &new_content).with_context(|| format!("write memory {}", path.display()))?;
    let _ = lock_file.unlock();
    let _ = fs::remove_file(&lock_path);
    Ok(())
}

/// Writes `content` to a temp file next to `path` and renames it
/// atomically, ensuring readers never see a partial write.
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Acquires an exclusive lock on `lock_path` (via `fs2`), runs `f`, and
/// releases the lock.  Used to serialise memory_summary.md updates across
/// concurrent agents.
pub fn with_file_lock<F, R>(lock_path: &Path, f: F) -> Result<R>
where
    F: FnOnce() -> Result<R>,
{
    let lock_file = File::create(lock_path)
        .with_context(|| format!("create lock file {}", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("acquire lock {}", lock_path.display()))?;
    let result = f();
    // Best-effort unlock; the OS will release when the process exits.
    let _ = lock_file.unlock();
    let _ = fs::remove_file(lock_path);
    result
}

/// Reads the full content of a memory file.
pub fn read_memory_file(path: &Path) -> Result<String> {
    validate_memory_path(path)?;
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).with_context(|| format!("read memory file {}", path.display()))
}

/// Reads the summary index truncated to a context-friendly limit.
///
/// Returns at most `SUMMARY_CONTEXT_LINES` lines or `SUMMARY_MAX_BYTES`
/// bytes — whichever is hit first.  The caller injects this into the
/// system prompt every turn.
pub fn read_summary_for_context(rara_home: &Path) -> Result<String> {
    let path = summary_path(rara_home)?;
    let raw = read_memory_file(&path)?;
    if raw.is_empty() {
        return Ok(String::new());
    }
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines.len() > SUMMARY_CONTEXT_LINES {
        lines.truncate(SUMMARY_CONTEXT_LINES);
        lines.push("... (truncated)");
    }
    let body = lines.join("\n");
    if body.len() > SUMMARY_MAX_BYTES as usize {
        // Truncate at byte boundary, avoiding mid-codepoint split
        let mut end = SUMMARY_MAX_BYTES as usize;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        return Ok(format!("{}...", &body[..end]));
    }
    Ok(body)
}

/// Rejects paths that escape the memory directory.
pub fn validate_memory_path(path: &Path) -> Result<()> {
    let canonical = if path.exists() {
        path.canonicalize()
            .with_context(|| format!("resolve memory path {}", path.display()))?
    } else {
        let parent = path.parent().unwrap_or(path);
        let canonical_parent = parent
            .canonicalize()
            .with_context(|| format!("resolve memory parent {}", path.display()))?;
        canonical_parent.join(path.file_name().unwrap_or(std::ffi::OsStr::new("")))
    };
    let in_memory_dir = canonical.components().any(|c| {
        use std::ffi::OsStr;
        c.as_os_str() == OsStr::new("memory")
    });
    if !in_memory_dir {
        bail!("memory path not under memory root: {}", path.display());
    }
    Ok(())
}

/// Returns the path to the memory summary index file.
///
/// On first access after upgrading from an older release, automatically
/// renames the legacy `summary.md` (and `summary.lock`) to the canonical
/// `memory_summary.md` names when the new file does not already exist.
pub fn summary_path(rara_home: &Path) -> Result<PathBuf> {
    let dir = ensure_memory_dir(rara_home)?;
    let new_path = dir.join("memory_summary.md");
    let old_path = dir.join("summary.md");

    if old_path.exists()
        && !new_path.exists()
        && let Err(e) = fs::rename(&old_path, &new_path)
    {
        log::warn!(
            "failed to migrate {} → {}: {e}",
            old_path.display(),
            new_path.display()
        );
    }

    let old_lock = dir.join("summary.lock");
    if old_lock.exists() {
        let _ = fs::remove_file(&old_lock);
    }

    Ok(new_path)
}

/// Maximum size for memory_summary.md before condensing old entries.
pub const SUMMARY_MAX_BYTES: u64 = 5 * 1024;

/// Maximum lines to read into context.
pub const SUMMARY_CONTEXT_LINES: usize = 200;

/// RARA-adapted memory read-path template (inspired by Codex `read_path.md`).
/// Prepended to summary content when the summary is non-empty.
pub const MEMORY_READ_PATH_HEADER: &str = "\
## Memory

You have access to a memory folder with guidance from prior runs. It can
save time and help you stay consistent. Use it whenever it is likely to help.

**Decision boundary**: Skip memory only when the request is clearly
self-contained (current time, trivial translation, one-line shell command).
Use memory by default when the query mentions workspace files/paths in the
summary below, asks for prior context, or is ambiguous and could depend on
earlier project decisions. If unsure, do a quick memory pass.

**Memory layout**:
- `memory_summary.md` (provided below; do NOT open again) — index of session pointers
- `global.md` — global project preferences and conventions
- `sessions/<id>.md` — per-session summaries of key decisions and outcomes

**Quick memory pass**: Skim the summary below for task-relevant keywords, then
use the `search_memory` tool to find specific details. Cite memory sources
with format `<file>:<line_start>-<line_end>|note=[how memory was used]`.

**Updating memory**: You can write to `memory.md` or create session files ONLY
when explicitly asked. Write additions under `~/.rara/memory/sessions/` or
append to `global.md`.

========= MEMORY SUMMARY BEGINS =========";

pub const MEMORY_READ_PATH_FOOTER: &str = "========= MEMORY SUMMARY ENDS =========";

/// Returns the full memory section for the system prompt, including
/// read-path instructions and truncated summary content.
pub fn read_memory_section(rara_home: &Path) -> String {
    let summary = match read_summary_for_context(rara_home) {
        Ok(s) if !s.is_empty() => s,
        _ => return String::new(),
    };
    format!(
        "{}\n{}\n{}",
        MEMORY_READ_PATH_HEADER, summary, MEMORY_READ_PATH_FOOTER
    )
}

/// Updates the summary index with a new session entry in Claude-style
/// one-line pointer format:
///
/// ```text
/// - [Session abc123](sessions/abc123.md) — Refactored auth module
/// ```
///
/// Acquires an exclusive lock on `memory_summary.lock`, so concurrent agents
/// serialise their index updates.
pub fn update_summary(rara_home: &Path, session_id: &str, topics: &str) -> Result<()> {
    let path = summary_path(rara_home)?;
    validate_memory_path(&path)?;
    let lock_path = path.with_extension("memory_summary.lock");

    with_file_lock(&lock_path, || {
        let mut content = if path.exists() {
            fs::read_to_string(&path).unwrap_or_default()
        } else {
            String::from("# Memory Summary\n\n")
        };

        // Format as Claude-style one-line pointers
        let filename = format!("sessions/{}.md", session_id);
        for topic in topics.lines() {
            let topic = topic.trim();
            if topic.is_empty() {
                continue;
            }
            content.push_str(&format!(
                "- [Session {}]({}) — {}\n",
                session_id, filename, topic
            ));
        }
        content.push('\n');

        // Truncate oldest entries when over the byte limit
        if content.len() as u64 > SUMMARY_MAX_BYTES {
            content = condense_old_entries(&content);
        }

        atomic_write(&path, &content)
            .with_context(|| format!("write summary {}", path.display()))?;
        Ok(())
    })
}

/// Condenses the oldest session entries when the summary exceeds 5KB.
/// Keeps the most recent entries and drops the oldest lines (FIFO).
pub fn condense_old_entries(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let header = lines.first().copied().unwrap_or("# Memory Summary");
    let entries: Vec<&str> = lines
        .iter()
        .filter(|l| l.starts_with("- [") && l.contains("](") && l.contains(") — "))
        .copied()
        .collect();
    if entries.is_empty() {
        return content.to_string();
    }
    // Walk newest-to-oldest, collect entries that fit in 5KB, then
    // reverse for correct chronological order.
    let mut kept: Vec<String> = Vec::new();
    let mut byte_count = header.len() + 2; // header + "\n\n"
    for entry in entries.iter().rev() {
        let entry_text = format!("{}\n", entry);
        byte_count += entry_text.len();
        if byte_count > SUMMARY_MAX_BYTES as usize {
            break;
        }
        kept.push(entry_text);
    }
    kept.reverse();
    let mut condensed = format!("{}\n\n", header);
    if kept.len() < entries.len() {
        condensed.push_str("... (older entries truncated)\n");
    }
    condensed.push_str(&kept.concat());
    condensed
}

/// A single search hit from memory files.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemorySearchHit {
    pub path: String,
    pub snippet: String,
}

/// Searches all memory files with a unified `rg`-first strategy.
pub async fn search_memory(query: &str, rara_home: &Path) -> Result<Vec<MemorySearchHit>> {
    let mut results = Vec::new();

    if let Ok(rg_hits) = rg_search_memory(query, rara_home) {
        results.extend(rg_hits);
    }

    Ok(merge_memory_results(results))
}

/// Runs rg over all `.md` files in the memory directory.
pub fn rg_search_memory(
    query: &str,
    rara_home: &Path,
) -> Result<Vec<MemorySearchHit>, std::io::Error> {
    let dir = memory_dir(rara_home);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut cmd = std::process::Command::new("rg");
    let output = cmd
        .args([
            "--no-heading",
            "--with-filename",
            "--line-number",
            "--max-count=10",
            "--glob",
            "*.md",
            "-F",
            query,
        ])
        .current_dir(&dir)
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    for line in stdout.lines() {
        if let Some((path, snippet)) = line.split_once(':') {
            results.push(MemorySearchHit {
                path: path.to_string(),
                snippet: snippet.to_string(),
            });
        }
    }
    Ok(results)
}

/// Deduplicates text results by snippet prefix.
pub fn merge_memory_results(text: Vec<MemorySearchHit>) -> Vec<MemorySearchHit> {
    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();
    for hit in text {
        if seen.insert(hit.snippet.chars().take(80).collect::<String>()) {
            results.push(hit);
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".rara").join("memory");
        fs::create_dir_all(&mem_dir).unwrap();
        let path = mem_dir.join("test.md");
        write_memory(&path, "hello world").unwrap();
        let content = read_memory_file(&path).unwrap();
        assert!(content.contains("hello world"));
    }

    #[test]
    fn atomic_write_keeps_prior_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".rara").join("memory");
        fs::create_dir_all(&mem_dir).unwrap();
        let path = mem_dir.join("test.md");
        write_memory(&path, "line 1").unwrap();
        write_memory(&path, "line 2").unwrap();
        let content = read_memory_file(&path).unwrap();
        assert!(content.contains("line 1"));
        assert!(content.contains("line 2"));
    }

    #[test]
    fn summary_entry_format_is_claude_style() {
        let dir = tempfile::tempdir().unwrap();
        let rara_home = dir.path().join(".rara");
        fs::create_dir_all(&rara_home).unwrap();
        update_summary(
            &rara_home,
            "session-1",
            "Fixed auth bug\nAdded rate limiting",
        )
        .unwrap();
        let content = read_summary_for_context(&rara_home).unwrap();
        assert!(content.contains("- [Session session-1](sessions/session-1.md) — Fixed auth bug"));
        assert!(content.contains("Added rate limiting"));
    }

    #[test]
    fn summary_respects_byte_limit() {
        let dir = tempfile::tempdir().unwrap();
        let rara_home = dir.path().join(".rara");
        fs::create_dir_all(&rara_home).unwrap();
        for i in 0..200 {
            let id = format!("session-{}", i);
            update_summary(&rara_home, &id, "Topic").unwrap();
        }
        let summary_path = summary_path(&rara_home).unwrap();
        let raw = fs::read_to_string(&summary_path).unwrap();
        assert!(raw.len() <= SUMMARY_MAX_BYTES as usize + 200); // small margin
    }

    fn rg_available() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn search_memory_returns_rg_hits() {
        if !rg_available() {
            return; // skip when rg is not installed
        }
        let dir = tempfile::tempdir().unwrap();
        let rara_home = dir.path().join(".rara");
        fs::create_dir_all(rara_home.join("memory").join("sessions")).unwrap();
        let session_path = rara_home.join("memory").join("sessions").join("test.md");
        fs::write(&session_path, "remember: use cargo fmt before commit").unwrap();

        let hits = search_memory("cargo fmt", &rara_home).await.unwrap();
        assert!(hits.iter().any(|h| h.snippet.contains("cargo fmt")));
    }
}
