# File Search Memory Selection

## What Changed

- Added focused coverage that proves `file_search` retrieval candidates pass
  through the shared `MemorySelection` ranking and budget path.
- Updated the file-search feature spec to record the current paths-only
  adapter behavior.

## Why

File search is now part of context routing through precomputed
`RetrievalCandidate` values. The contract needs to stay explicit because the
adapter surfaces matched paths for selection and observability, but it does not
read file contents or inject excerpts.

## Trade-Offs

- The adapter keeps file-search candidates cheap and cache-stable by charging
  only path/provenance text.
- File contents remain out of scope until an excerpt-selection step can own
  token budgets, provenance, and prompt-cache impact.

## Remaining Work

- Add a session-style incremental search surface for large-workspace TUI
  pickers.
- Design a separate excerpt-selection contract before allowing file contents to
  enter the prompt automatically.
