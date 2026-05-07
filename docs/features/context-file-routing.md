# Context File Routing

**Status**: spec
**Date**: 2026-05-07
**Depends on**: `crates/file-search`, `docs/features/memory-selection.md`

## Overview

`crates/file-search` provides fuzzy file search via `search_files(pattern, roots, opts)`.
Currently it is only used by the TUI file picker (`src/tools/file.rs` `list_files`).

This spec defines how file-search results become context candidates that flow through
the existing `memory_selection` pipeline, alongside retrieved memory and workspace
prompt sources — without automatic injection.

---

## Design Goals

1. **Candidates only** — file-search produces `MemorySelectionItemContextEntry`
   candidates; the existing selection logic decides which to include.

2. **Provenance** — every candidate carries an explicit origin (`file_search`,
   match score, file path) for auditability.

3. **Token budget** — each candidate estimates its token cost. The
   `MemorySelection` budget allocator decides how many fit.

4. **Stable ordering** — candidates are ordered deterministically by (score
   descending, path ascending) so repeated runs produce the same list.

5. **Single interface** — the same candidate provider serves both the TUI picker
   (listFiles) and the context routing path.

---

## Candidate Model

### `FileSearchCandidate`

```rust
/// A file match returned by the file-search engine, ready for context routing.
#[derive(Debug, Clone)]
pub struct FileSearchCandidate {
    /// Absolute workspace-relative display path.
    pub path: String,
    /// Full absolute path on disk.
    pub full_path: String,
    /// Match score from nucleo (0.0–1.0).
    pub score: f64,
    /// Whether this was a fuzzy-name match or a content-grep match.
    pub match_type: FileMatchType,
    /// Estimated token count for the file content (capped).
    pub token_budget: usize,
    /// Human-readable provenance label.
    pub provenance: String,
}

#[derive(Debug, Clone, Copy)]
pub enum FileMatchType {
    /// Matched on file name.
    Name,
    /// Matched on file content (future: content_grep).
    Content,
}
```

### Conversion to `MemorySelectionItemContextEntry`

```rust
impl FileSearchCandidate {
    fn to_context_entry(&self, order: usize) -> MemorySelectionItemContextEntry {
        MemorySelectionItemContextEntry {
            order,
            kind: "file_search".to_string(),
            label: self.path.clone(),
            detail: self.provenance.clone(),
            selection_reason: format!(
                "candidate from file search (score {:.3}, match_type {:?})",
                self.score, self.match_type
            ),
            budget_impact_tokens: Some(self.token_budget),
            priority: (self.score * 1000.0) as usize,
            selectable: true,
            dropped_reason: None,
        }
    }
}
```

### Provenance format

```
provenance = "file_search(name_match, score=0.82, root=/workspace)"
```

---

## Provider Interface

### `FileSearchCandidateProvider`

```rust
/// Provides file-search candidates for both TUI picker and context routing.
pub struct FileSearchCandidateProvider {
    /// workspace root directory
    workspace_root: PathBuf,
    /// Whether to respect .gitignore
    respect_gitignore: bool,
}

impl FileSearchCandidateProvider {
    /// Search for files matching `query`, returning scored candidates.
    /// Capped at `max_results`.
    pub fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Vec<FileSearchCandidate>;

    /// Produce context entries ready for MemorySelection.
    /// Each entry carries provenance, budget, and a stable order.
    pub fn context_candidates(
        &self,
        query: &str,
        max_results: usize,
    ) -> Vec<MemorySelectionItemContextEntry>;
}
```

### Token budget estimation

```rust
fn estimate_file_token_budget(path: &Path, max_bytes: usize) -> usize {
    match std::fs::read_to_string(path) {
        Ok(content) => estimate_text_tokens(
            &content.chars().take(max_bytes).collect::<String>()
        ),
        Err(_) => 0,
    }
}
```

Hueristic: read up to `MAX_FILE_CANDIDATE_BYTES` (default 8 KiB) of the file, count
tokens.  The budget allocator in `memory_selection` subtracts this from the
`selection_budget`.

---

## Integration Point

### `memory_selection()` signature (updated)

```rust
pub(crate) fn memory_selection(
    prompt_sources: &[PromptSource],
    plan_explanation: Option<&str>,
    plan_steps: &[(PlanStepStatus, String)],
    pending_interactions: &[RuntimeInteractionInput],
    compacted_history: &[CompactionSourceContextEntry],
    history: &[Message],
    session_id: &str,
    vdb_uri: Option<&str>,
    retrieved_memory_candidates: &[RetrievedMemoryCandidate],
    selection_budget: Option<usize>,
    // NEW: file-search candidates from the provider
    file_search_candidates: &[MemorySelectionItemContextEntry],
) -> MemorySelectionContextView;
```

### Candidate flow

```
rara_file_search::search_files("pattern", [workspace_root], opts)
        │
        ▼
FileSearchCandidateProvider::context_candidates(query, max)
        │  ┌─ provenance: "file_search(name_match, score=0.82)"
        │  ├─ token_budget: estimate_file_token_budget(path)
        │  └─ priority: score-derived
        ▼
MemorySelectionItemContextEntry (kind = "file_search")
        │
        ▼
memory_selection()                ← existing budget allocator
        │
        ▼
MemorySelectionContextView.available_items   ← "file_search" category
        │
        ▼
ContextAssembler                 ← decides whether to include
```

---

## What NOT to do

- ❌ Do NOT auto-inject file contents into the prompt.
- ❌ Do NOT modify `PromptSource` or the workspace prompt source list.
- ❌ Do NOT bypass the `memory_selection` budget allocator.
- ❌ Do NOT add a separate "file search" prompt block — candidates are just
  candidates.
- ❌ Do NOT break the existing `listFiles` TUI picker — it continues to use
  `FileSearchCandidateProvider::search()` directly.

---

## Ordering

Candidates are ordered by:
1. Score descending (higher match first)
2. Path ascending (lexicographic tiebreaker)

This produces a deterministic list that survives repeated calls.

---

## Future

- `grep_files()` for content-search candidates (match_type = Content)
- `FileSearchCache` with incremental re-scan (session-style)
- TUI inline file-search picker (Ctrl-F in composer)
- Context-budget-aware truncation hints (“this file is too large; read
  offset=0 limit=100 first”)

---

## References

- `crates/file-search/src/lib.rs` — fuzzy file search engine
- `docs/features/memory-selection.md` — candidate selection pipeline
- `src/context/memory_selection.rs` — `memory_selection()` implementation
- `src/context/assembler.rs` — `assemble_runtime()` caller
