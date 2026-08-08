# Session and Global File-Based Memory

## Problem

The runtime needs reliable short-term memory across restarts without depending
on embedded embeddings or a vector database. The local path should resemble
Claude/Codex-style file memory: simple files first, explicit retrieval when
needed, and no hidden semantic index.

## Scope

- Session-scoped context shards under the workspace rollout directory.
- Workspace/global memory files under the local memory root.
- Local text search over files and persisted `MemoryRecord` records.
- Summary-driven or policy-driven injection through `MemorySelection`.

## Non-Goals

- No LanceDB collections.
- No bundled embedding model.
- No vector search.
- No local semantic full-text replacement beyond deterministic text search.

Official Mem owns semantic and cross-tool memory.

## Architecture

Local memory uses two storage lanes:

```text
<workspace>/.rara/memory/              local file memory handle
<workspace>/.rara/memories/records.json durable MemoryRecord records
<workspace>/.rara/rollouts/<id>/context.jsonl session context checkpoints
```

`search_memory` searches local memory files. `MemoryStore::search` searches
persisted records with local text matching. `retrieve_session_context` searches
session context shards directly.

## Contracts

- File memory remains usable without network access, model downloads, or vector
  table setup.
- Session context append/search must not require embedding vectors.
- Local search should return enough path/detail metadata for the runtime to
  explain selected and dropped context.
- Semantic recall should be implemented by official Mem integration, not by a
  local fallback database.

## Validation Matrix

| Contract | Validation |
|----------|------------|
| File search has no vector dependency | `cargo test memory_store -- --nocapture` |
| Session shard search has no vector dependency | `cargo test session_context -- --nocapture` |
| Context assembly still ranks retrieved local memory | `cargo test context::assembler -- --nocapture` |

## Source Journals

- [2026-08-08-remove-embedded-vector-memory.md](../journal/2026-08-08-remove-embedded-vector-memory.md)
