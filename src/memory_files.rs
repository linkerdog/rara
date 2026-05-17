//! Durable session and global memory files under `~/.rara/memory/`.
//!
//! Implements Phase 1 of `docs/features/session-global-memory.md`:
//! session-scoped `.md` files and a global `global.md` file with
//! path-traversal protection.

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
    // Platform-agnostic: check that "memory" appears as a full path component.
    // `components()` yields platform-independent path segments, so "memory"
    // matches as a complete directory name, not a substring.
    let in_memory_dir = canonical.components().any(|c| {
        use std::ffi::OsStr;
        c.as_os_str() == OsStr::new("memory")
    });
    if !in_memory_dir {
        bail!("memory path not under memory root: {}", path.display());
    }
    Ok(())
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
}
