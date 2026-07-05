# Context File Routing

**Status**: done
**Date**: 2026-05-07
**Depends on**: `crates/file-search`, `docs/features/memory-selection.md`

## Overview

`crates/file-search` provides fuzzy file search via `search_files(pattern, roots, opts)`.
It is the shared backend for explicit file-selection flows such as the TUI file
picker and the `list_files` tool.

This spec also defines the narrow automatic-retrieval bridge: when enabled,
file-search results become low-priority paths-only `RetrievalCandidate` values
that flow through the existing `memory_selection` pipeline. They expose path and
provenance only; they do not read or inject file contents.

---

## Design Goals

1. **Shared backend** — explicit TUI file suggestions and `list_files` use the
   same `rara-file-search` backend as context routing.

2. **Paths only** — automatic retrieval produces `RetrievalCandidate` values
   that contain path/provenance metadata only. File contents stay behind
   explicit tools such as `read_file`.

3. **Optional routing** — `context_file_search = "paths_only"` is the default;
   `context_file_search = "off"` disables automatic file-search candidates
   without affecting explicit picker/list-files flows.

4. **Provenance** — every candidate carries an explicit origin (`file_search`,
   match score, file path) for auditability.

5. **Token budget** — each automatic candidate estimates the cost of its
   manifest text only. It must not charge or imply file-content injection.

6. **Stable ordering** — candidates are ordered deterministically by (score
   descending, path ascending) so repeated runs produce the same list.

---

## Candidate Model

### `FileSearchCandidate`

```rust
/// A file match returned by the file-search engine, ready for context routing.
#[derive(Debug, Clone)]
pub struct FileSearchCandidate {
    /// Workspace-relative display path.
    pub path: String,
    /// Match score from nucleo (0.0–1.0).
    pub score: f64,
    /// Estimated token count for the path/provenance candidate.
    pub token_budget: usize,
    /// Human-readable provenance label.
    pub provenance: String,
}
```

### Conversion to `RetrievalCandidate`

```rust
impl FileSearchCandidate {
    fn to_retrieval_candidate(&self, rank: usize) -> RetrievalCandidate {
        RetrievalCandidate {
            kind: "file_search".into(),
            scope: "workspace".into(),
            label: self.path.clone(),
            detail: format!("{}; paths_only; content_not_read", self.provenance),
            priority: 80 + rank,
            budget_impact_tokens: Some(self.token_budget),
            selection_reason: format!(
                "paths-only candidate from file search (score {:.3}); file contents were not read",
                self.score
            ),
            // source.source_path is the same workspace-relative path.
            // Other fields omitted here for brevity.
        }
    }
}
```

### Provenance format

```
provenance = "file_search(name_match, score=0.820)"
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

    /// Produce paths-only retrieval candidates ready for MemorySelection.
    /// Each candidate carries provenance, path budget, and stable order.
    pub fn retrieval_candidates(
        &self,
        query: &str,
        max_results: usize,
    ) -> Vec<RetrievalCandidate>;
}
```

### Token budget estimation

```rust
fn estimate_path_candidate_tokens(path: &Path) -> usize {
    let path_tokens = path.to_string_lossy().len().div_ceil(4);
    path_tokens.max(1)
}
```

Heuristic: charge only the path/provenance manifest. The provider must not open
the file for automatic retrieval.

## Configuration

```toml
# Default. Explicit file picker/list_files behavior is unchanged.
context_file_search = "paths_only"

# Disable only automatic file-search retrieval candidates.
context_file_search = "off"
```

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
    file_search_candidates: &[RetrievalCandidate],
    graph_context_candidates: &[RetrievalCandidate],
) -> MemorySelectionContextView;
```

### Candidate flow

```
rara_file_search::search_files("pattern", [workspace_root], opts)
        │
        ▼
FileSearchCandidateProvider::retrieval_candidates(query, max)
        │  ┌─ provenance: "file_search(name_match, score=0.82)"
        │  ├─ token_budget: estimate_path_candidate_tokens(path)
        │  └─ priority: low-priority paths-only source
        ▼
RetrievalCandidate (kind = "file_search", source_path = path)
        │
        ▼
memory_selection()                ← existing budget allocator
        │
        ▼
MemorySelectionContextView.available_items   ← "file_search" category
        │
        ▼
ContextAssembler                 ← may expose selected manifest only
```

---

## What NOT to do

- ❌ Do NOT auto-inject file contents into the prompt.
- ❌ Do NOT read file contents while producing automatic file-search
  candidates.
- ❌ Do NOT modify `PromptSource` or the workspace prompt source list.
- ❌ Do NOT bypass the `memory_selection` budget allocator.
- ❌ Do NOT add a separate "file search" prompt block — candidates are just
  candidates.
- ❌ Do NOT break the existing `list_files` tool or TUI picker — they continue to use
  `FileSearchCandidateProvider::search()` directly.

---

## Ordering

Candidates are ordered by:
1. Score descending (higher match first)
2. Path ascending (lexicographic tiebreaker)

This produces a deterministic list that survives repeated calls.

---

## Future

- `grep_files()` for explicit content-search tools, not automatic injection.
- `FileSearchCache` with incremental re-scan (session-style)
- TUI inline file-search picker (Ctrl-F in composer)
- Context-budget-aware truncation hints (“this file is too large; read
  offset=0 limit=100 first”)

---

## References

> **Note**: The integration point diagram above shows architectural relationships, not
> literal function signatures. For current API contracts, see the respective source files.

- `crates/file-search/src/lib.rs` — fuzzy file search engine
- `docs/features/memory-selection.md` — candidate selection pipeline
- `src/context/memory_selection.rs` — `memory_selection()` implementation
- `src/context/assembler.rs` — `assemble_runtime()` caller
