# 2026-05-07 · Retrieval Provider Boundary

## Context

The retrieval orchestration view already exposed selected, available, dropped,
provider status, and budget rollups. The remaining structural gap was that
`MemorySelection` still accepted source-specific inputs directly: retrieved
memory candidates, file-search candidates, thread history, and vector-store
status were merged inside the selector.

## Implementation

- Added `RetrievalRequest` and `RetrievalSourceProvider` as the first source
  provider boundary.
- Added current-source adapters for direct retrieved memory, retrieval tool
  results, thread history, vector-store slot, and precomputed file-search
  candidates.
- Changed file search to produce typed `RetrievalCandidate` values before the
  compatibility `MemorySelectionItemContextEntry` conversion.
- Changed discretionary `memory_selection()` input to a normalized
  `RetrievalCandidate` slice.
- Kept selected retrieved-memory prompt injection unchanged: it remains
  volatile per-turn context prepended to the latest user request and is not
  persisted into stable prompt sources.

## Validation

- `cargo test context::retrieval_provider -- --nocapture`
- `cargo test context::memory_selection -- --nocapture`
- `cargo test context::file_search_provider -- --nocapture`

## Follow-up

- Add MCP resource providers through the same source-provider contract.
- Add hook and graph providers after the current memory/session/file path
  remains stable.
- Add structured provider failure and skip reporting before auxiliary-model
  candidate compression.
