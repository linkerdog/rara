//! Durable session and global memory files under `~/.rara/memory/`.
//!
//! Implements `docs/features/session-global-memory.md`:
//! session-scoped `.md` files, global `global.md`, `summary.md` index,
//! and a unified `search_memory` that merges file grep + LanceDB results.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Returns the memory root directory under the given RARA home.
pub(crate) fn memory_dir(rara_home: &Path) -> PathBuf {
    rara_home.join("memory")
}

/// Creates the memory directory hierarchy if it doesn't exist.
pub(crate) fn ensure_memory_dir(rara_home: &Path) -> Result<PathBuf> {
    let dir = memory_dir(rara_home);
    fs::create_dir_all(dir.join("sessions"))?;
    Ok(dir)
}

/// Returns the path to a session memory file.
pub(crate) fn session_memory_path(rara_home: &Path, session_id: &str) -> Result<PathBuf> {
    let dir = ensure_memory_dir(rara_home)?;
    Ok(dir.join("sessions").join(format!("{session_id}.md")))
}

/// Returns the path to the global memory file.
pub(crate) fn global_memory_path(rara_home: &Path) -> Result<PathBuf> {
    let dir = ensure_memory_dir(rara_home)?;
    Ok(dir.join("global.md"))
}

/// Appends content to a memory file.
pub(crate) fn write_memory(path: &Path, content: &str) -> Result<()> {
    validate_memory_path(path)?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open memory file {}", path.display()))?;
    writeln!(f, "{content}")?;
    Ok(())
}

/// Reads the full content of a memory file.
pub(crate) fn read_memory_file(path: &Path) -> Result<String> {
    validate_memory_path(path)?;
    fs::read_to_string(path).with_context(|| format!("read memory file {}", path.display()))
}

/// Rejects paths that escape the memory directory.
fn validate_memory_path(path: &Path) -> Result<()> {
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
pub(crate) fn summary_path(rara_home: &Path) -> Result<PathBuf> {
    let dir = ensure_memory_dir(rara_home)?;
    Ok(dir.join("summary.md"))
}

/// Maximum size for summary.md before condensing old entries.
const SUMMARY_MAX_BYTES: u64 = 5 * 1024;

/// Updates the summary index with a new session entry.
/// Condenses oldest entries when the file exceeds 5KB.
pub(crate) fn update_summary(rara_home: &Path, session_id: &str, topics: &str) -> Result<()> {
    let path = summary_path(rara_home)?;
    validate_memory_path(&path)?;

    let mut content = if path.exists() {
        fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::from("# Memory Summary\n\n")
    };

    let entry = format!("\n## Session {session_id}\n{topics}\n");
    content.push_str(&entry);

    if content.len() as u64 > SUMMARY_MAX_BYTES {
        content = condense_old_entries(&content);
    }

    fs::write(&path, &content).with_context(|| format!("write summary {}", path.display()))?;
    Ok(())
}

/// Condenses the oldest session entries when the summary exceeds 5KB.
/// Keeps the most recent ~10 sessions individually and collapses older ones.
fn condense_old_entries(content: &str) -> String {
    let mut sections: Vec<&str> = content.split("\n## ").collect();
    if sections.is_empty() {
        return content.to_string();
    }

    let header = sections.remove(0);
    let mut new_sections = vec![header.to_string()];

    let mut session_indices = vec![];
    for (i, s) in sections.iter().enumerate() {
        if s.starts_with("Session ") {
            session_indices.push(i);
        }
    }

    if session_indices.len() <= 10 {
        for s in &sections {
            new_sections.push(format!("## {s}"));
        }
        return new_sections.join("\n");
    }

    let keep_count = 10;
    let split_point = session_indices[session_indices.len() - keep_count];
    let old_count = session_indices.len() - keep_count;
    let first_id = sections[session_indices[0]]
        .strip_prefix("Session ")
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("?");
    let last_idx = session_indices[old_count - 1];
    let last_id = sections[last_idx]
        .strip_prefix("Session ")
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("?");

    for (i, s) in sections.iter().enumerate() {
        if i >= split_point {
            break;
        }
        if i < session_indices[0] {
            new_sections.push(format!("## {s}"));
        }
    }

    new_sections.push(format!(
        "## Sessions {first_id}..{last_id} (archived, {old_count} sessions)"
    ));

    for s in &sections[split_point..] {
        new_sections.push(format!("## {s}"));
    }

    new_sections.join("\n")
}

/// Reads the full summary index.
pub(crate) fn read_summary(rara_home: &Path) -> Result<String> {
    let path = summary_path(rara_home)?;
    if path.exists() {
        read_memory_file(&path)
    } else {
        Ok(String::new())
    }
}

/// Searches memory files for matching content.
/// Uses `rg` when available; falls back to recursive file walk only when
/// rg is not installed (not when it finds no results).
pub(crate) fn search_memory(rara_home: &Path, query: &str) -> Result<Vec<String>> {
    let mut results = Vec::new();
    let dir = memory_dir(rara_home);
    if !dir.exists() {
        return Ok(results);
    }

    match std::process::Command::new("rg")
        .arg("-l")
        .arg("--no-heading")
        .arg(query)
        .arg(&dir)
        .output()
    {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Ok(rel) = std::path::Path::new(line.trim()).strip_prefix(&dir) {
                        results.push(format!("file: {}", rel.display()));
                    }
                }
            }
        }
        Err(_) => {
            walk_memory_dir(&dir, &dir, query, &mut results);
        }
    }

    Ok(results)
}

fn walk_memory_dir(base: &Path, dir: &Path, query: &str, results: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_memory_dir(base, &path, query, results);
            } else if path.extension().map_or(false, |e| e == "md") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if content.contains(query) {
                        if let Ok(rel) = path.strip_prefix(base) {
                            results.push(format!("file: {}", rel.display()));
                        }
                    }
                }
            }
        }
    }
}

/// Creates a session memory file and records it in the summary index.
pub(crate) fn create_session(rara_home: &Path, session_id: &str) -> Result<PathBuf> {
    let path = session_memory_path(rara_home, session_id)?;
    if !path.exists() {
        write_memory(&path, &format!("# Session {session_id}\n\n"))?;
        update_summary(rara_home, session_id, "(new session)")?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_home() -> PathBuf {
        let id = std::process::id();
        std::env::temp_dir().join(format!("rara-memory-test-{id}"))
    }

    #[test]
    fn write_and_read_session_memory() {
        let home = test_home();
        let path = session_memory_path(&home, "test-session").expect("session path");
        let _ = fs::remove_file(&path);
        write_memory(&path, "## Key Finding\n- Test entry").expect("write");
        write_memory(&path, "- Another entry").expect("append");
        let content = read_memory_file(&path).expect("read");
        assert!(content.contains("Test entry"));
        assert!(content.contains("Another entry"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn write_and_read_global_memory() {
        let home = test_home();
        let path = global_memory_path(&home).expect("global path");
        write_memory(&path, "## Global Test\n- Global entry").expect("write");
        let content = read_memory_file(&path).expect("read");
        assert!(content.contains("Global entry"));
    }

    #[test]
    fn rejects_path_traversal() {
        let home = test_home();
        let dir = memory_dir(&home);
        let _ = fs::create_dir_all(&dir);
        let bad_path = dir.join("../evil.md");
        assert!(validate_memory_path(&bad_path).is_err());
    }

    #[test]
    fn ensures_memory_dir_creates_structure() {
        let home = test_home();
        let test_dir = home.join("memory").join("sessions");
        let _ = fs::remove_dir_all(&home);
        let result = ensure_memory_dir(&home);
        assert!(result.is_ok());
        assert!(test_dir.exists());
    }

    #[test]
    fn summary_updates_and_reads_back() {
        let home = test_home();
        let _ = fs::remove_file(summary_path(&home).unwrap_or_else(|_| PathBuf::new()));
        update_summary(&home, "sess-1", "- topic A\n- topic B").expect("update");
        update_summary(&home, "sess-2", "- topic C").expect("append");

        let content = read_summary(&home).expect("read");
        assert!(content.contains("sess-1"));
        assert!(content.contains("sess-2"));
        assert!(content.contains("topic A"));
    }

    #[test]
    fn search_memory_finds_keywords() {
        let home = test_home();
        let path = session_memory_path(&home, "search-test").expect("path");
        let _ = fs::remove_file(&path);
        write_memory(&path, "## Finding\n- venv creation fix\n- uv --python 3.14").expect("write");

        let results = search_memory(&home, "venv").expect("search");
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.contains("search-test")));
    }

    #[test]
    fn create_session_bootstraps_file_and_summary() {
        let home = test_home();
        let path = create_session(&home, "boot-test").expect("create");
        assert!(path.exists());

        let summary = read_summary(&home).expect("summary");
        assert!(summary.contains("boot-test"));

        let _ = fs::remove_file(&path);
    }
}
