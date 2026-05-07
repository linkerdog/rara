//! File-search candidate provider for context routing.
//!
//! Wraps `rara_file_search::search_files()` to produce
//! `MemorySelectionItemContextEntry` candidates that flow through the
//! existing `memory_selection` budget allocator.
//!
//! Candidates are NOT auto-injected — the selection logic decides
//! which files to include based on token budget and priority.

use std::path::{Path, PathBuf};

use anyhow::Context;
use rara_file_search::FileSearchOptions;

use crate::context::runtime::MemorySelectionItemContextEntry;

/// A file match from the file-search engine, ready for context routing.
#[derive(Debug, Clone)]
pub struct FileSearchCandidate {
    /// Display path relative to workspace root.
    pub path: String,
    /// Match score from nucleo (0.0–1.0).
    pub score: f64,
    /// Estimated token count for the file content (capped).
    pub token_budget: usize,
    /// Human-readable provenance label.
    pub provenance: String,
}

/// Provides file-search candidates for context routing and TUI picker.
pub struct FileSearchCandidateProvider {
    workspace_root: PathBuf,
    respect_gitignore: bool,
}

impl FileSearchCandidateProvider {
    pub fn new(workspace_root: PathBuf, respect_gitignore: bool) -> Self {
        Self {
            workspace_root,
            respect_gitignore,
        }
    }

    /// Search for files matching `query`, returning scored candidates.
    /// Capped at `max_results`.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<FileSearchCandidate> {
        let options = FileSearchOptions {
            limit: std::num::NonZero::new(max_results.max(1))
                .context("limit must be nonzero")
                .unwrap(),
            respect_gitignore: self.respect_gitignore,
            ..Default::default()
        };

        let results =
            rara_file_search::search_files(query, vec![self.workspace_root.clone()], options)
                .ok()
                .map_or(Vec::new(), |r| r.matches);

        results
            .into_iter()
            .map(|m| FileSearchCandidate {
                path: display_path(&self.workspace_root, &m.path),
                score: m.score as f64,
                token_budget: estimate_file_token_budget(&m.path),
                provenance: provenance_label(m.score as f64),
            })
            .collect()
    }

    /// Produce `MemorySelectionItemContextEntry` candidates for the
    /// `memory_selection` budget allocator.
    ///
    /// Each candidate carries provenance, budget, and a stable order
    /// (score descending, path ascending).
    pub fn context_candidates(
        &self,
        query: &str,
        max_results: usize,
    ) -> Vec<MemorySelectionItemContextEntry> {
        let mut candidates = self.search(query, max_results);

        // Stable ordering: score descending, path ascending
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });

        candidates
            .into_iter()
            .enumerate()
            .map(|(idx, c)| MemorySelectionItemContextEntry {
                order: idx + 1,
                kind: "file_search".to_string(),
                label: c.path,
                detail: c.provenance,
                selection_reason: format!("candidate from file search (score {:.3})", c.score),
                budget_impact_tokens: Some(c.token_budget),
                dropped_reason: None,
            })
            .collect()
    }
}

/// Strip workspace root prefix for display.
fn display_path(root: &Path, full: &Path) -> String {
    full.strip_prefix(root)
        .unwrap_or(full)
        .to_string_lossy()
        .to_string()
}

/// Build a provenance label for a file match.
fn provenance_label(score: f64) -> String {
    format!("file_search(name_match, score={:.3})", score)
}

/// Heuristic token estimate: read up to 8 KiB of the file, count chars / 4.
/// Returns 0 if the file cannot be read.
fn estimate_file_token_budget(path: &Path) -> usize {
    const MAX_BYTES: usize = 8192;
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let preview: String = content.chars().take(MAX_BYTES).collect();
            // Rough estimate: ~4 chars per token for English text.
            preview.len() / 4
        }
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use {super::*, tempfile};

    #[test]
    fn display_path_strips_root() {
        let root = Path::new("/workspace");
        let full = Path::new("/workspace/src/main.rs");
        assert_eq!(display_path(root, full), "src/main.rs");
    }

    #[test]
    fn estimate_file_token_budget_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "hello world").unwrap();
        let budget = estimate_file_token_budget(&path);
        assert!(budget > 0);
    }

    #[test]
    fn estimate_file_token_budget_missing() {
        let budget = estimate_file_token_budget(Path::new("/nonexistent/file.txt"));
        assert_eq!(budget, 0);
    }

    #[test]
    fn provider_produces_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let provider = FileSearchCandidateProvider::new(dir.path().to_path_buf(), false);
        let candidates = provider.search("hello", 10);
        assert!(!candidates.is_empty());
        assert_eq!(candidates[0].path, "hello.rs");
        assert!(candidates[0].score > 0.0);
        assert!(candidates[0].token_budget > 0);
        assert!(candidates[0].provenance.contains("file_search"));
    }

    #[test]
    fn context_candidates_have_stable_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "a").unwrap();
        std::fs::write(dir.path().join("b.rs"), "b").unwrap();

        let provider = FileSearchCandidateProvider::new(dir.path().to_path_buf(), false);
        let entries = provider.context_candidates(".rs", 10);
        // Both .rs files match; stable ordering by score then path
        assert_eq!(entries.len(), 2);
        assert!(entries[0].order < entries[1].order);
        // label is the display path
        assert!(entries.iter().all(|e| e.label.ends_with(".rs")));
        assert!(entries.iter().all(|e| e.kind == "file_search"));
    }
}
