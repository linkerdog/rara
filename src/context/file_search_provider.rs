//! File-search candidate provider for context routing.
//!
//! Wraps `rara_file_search::search_files()` to produce paths-only
//! `RetrievalCandidate` values that flow through the existing
//! `memory_selection` budget allocator.
//!
//! Candidates are NOT content injections. They only expose the matched path and
//! provenance so `/context` can explain why a file surfaced.

use std::path::{Path, PathBuf};

use anyhow::Context;
use rara_file_search::FileSearchOptions;

use crate::context::retrieval_provider::stable_retrieval_text_id;
use crate::context::runtime::{RetrievalCandidate, RetrievalSourceRef};

/// A file match from the file-search engine, ready for context routing.
#[derive(Debug, Clone)]
pub struct FileSearchCandidate {
    /// Display path relative to workspace root.
    pub path: String,
    /// Match score from nucleo (0.0–1.0).
    pub score: f64,
    /// Estimated token count for the path/provenance candidate.
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
            .map(|m| {
                let path = display_path(&self.workspace_root, &m.path);
                let token_budget = estimate_path_candidate_tokens(Path::new(&path));
                FileSearchCandidate {
                    path,
                    score: m.score as f64,
                    token_budget,
                    provenance: provenance_label(m.score as f64),
                }
            })
            .collect()
    }

    pub fn retrieval_candidates(&self, query: &str, max_results: usize) -> Vec<RetrievalCandidate> {
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
            .map(|(idx, c)| {
                let rank = idx + 1;
                RetrievalCandidate {
                    id: format!("file_search:{rank}:{}", stable_retrieval_text_id(&c.path)),
                    source: RetrievalSourceRef {
                        source_type: "file_search".to_string(),
                        source_id: None,
                        source_path: Some(c.path.clone()),
                        source_uri: None,
                        session_id: None,
                        thread_id: None,
                        workspace_id: None,
                    },
                    kind: "file_search".to_string(),
                    scope: "workspace".to_string(),
                    label: c.path,
                    detail: format!("{}; paths_only; content_not_read", c.provenance),
                    summary: None,
                    rank,
                    score: Some(c.score as f32),
                    priority: 80 + rank,
                    dedupe_key: None,
                    budget_impact_tokens: Some(c.token_budget),
                    selection_reason: format!(
                        "paths-only candidate from file search (score {:.3}); file contents were not read",
                        c.score
                    ),
                    availability_reason:
                        "available because fuzzy path search matched the current turn query; this candidate carries only the path and provenance"
                            .to_string(),
                    not_selected_reason:
                        "not selected after ranking this low-priority paths-only file-search candidate against the current memory-selection budget"
                            .to_string(),
                    selectable: true,
                }
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

/// Heuristic token estimate for a paths-only candidate.
fn estimate_path_candidate_tokens(path: &Path) -> usize {
    let path_tokens = path.to_string_lossy().len().div_ceil(4);
    path_tokens.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_path_strips_root() {
        let root = Path::new("/workspace");
        let full = Path::new("/workspace/src/main.rs");
        assert_eq!(display_path(root, full), "src/main.rs");
    }

    #[test]
    fn estimate_path_candidate_budget_does_not_read_file_contents() {
        let budget = estimate_path_candidate_tokens(Path::new("/nonexistent/file.txt"));
        assert!(budget > 0);
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
        assert_eq!(candidates[0].token_budget, 2);
        assert!(candidates[0].provenance.contains("file_search"));
    }

    #[test]
    fn retrieval_candidates_are_low_priority_paths_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "a").unwrap();
        std::fs::write(dir.path().join("b.rs"), "b").unwrap();

        let provider = FileSearchCandidateProvider::new(dir.path().to_path_buf(), false);
        let entries = provider.retrieval_candidates(".rs", 10);
        // Both .rs files match; stable ordering by score then path
        assert_eq!(entries.len(), 2);
        assert!(entries[0].rank < entries[1].rank);
        // label is the display path
        assert!(entries.iter().all(|e| e.label.ends_with(".rs")));
        assert!(entries.iter().all(|e| e.kind == "file_search"));
        assert!(entries.iter().all(|e| e.priority >= 80));
        assert!(entries.iter().all(|e| e.detail.contains("paths_only")));
        assert!(
            entries
                .iter()
                .all(|e| e.selection_reason.contains("contents were not read"))
        );
    }
}
