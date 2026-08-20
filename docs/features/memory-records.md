# Memory Records and Storage

## Problem

RARA needs durable local memory for facts, decisions, procedures, and session
context. The local runtime should remain simple and predictable: short-term
memory is stored as local text/JSON files, while semantic recall is delegated to
official Mem instead of an embedded vector database or bundled embedding model.

## Scope

- `MemoryRecord` remains the durable domain object for agent-authored memory.
- Records are persisted in local JSON companion files under the workspace/user
  memory directory.
- Search is local text search over stored records and file memory.
- Session context is persisted in per-session `context.jsonl` shards.
- TUI auto-memory extraction writes through `MemoryStore` using the active LLM
  backend for extraction only.

## Non-Goals

- No embedded vector store.
- No LanceDB runtime dependency.
- No bundled embedding sidecar or local embedding model bootstrap.
- No local semantic similarity ranking in the core runtime.
- No compatibility tools named `remember_experience` or `retrieve_experience`.

Semantic retrieval, graph expansion, and cross-tool durable knowledge belong to
official Mem. Local memory exists as a short-term and file-backed substrate.

## Division of Labor

Local memory and official Mem split ownership by durability and scope:

- **Local (`memory.md` + `MemoryRecord`s)**: workspace-scoped, short-term,
  file-backed substrate with deterministic plain-text search. It holds
  project-specific facts and decisions that only matter inside this workspace.
- **Official Mem (`distill-memory` skill / Nowledge Mem)**: the authority for
  durable, cross-tool, and cross-workspace knowledge that must survive the
  current thread or workspace.

The model-facing write surfaces carry this routing rule so the same durable
fact is not distilled into both stores: `update_project_memory` targets local
memory, and the Nowledge Mem `distill-memory` skill targets official Mem.

## Architecture

`MemoryStore` owns the local memory domain boundary:

- `insert` persists a complete `MemoryRecord`.
- `search` performs deterministic local text matching and returns ranked hits.
- `update`, `delete`, `get`, `set_pinned`, and `list_labels` operate on the
  persisted domain record file.
- `MemoryRetrievalOrchestrator` turns local record hits and session-shard hits
  into ranked `MemorySelection` candidates.

`MemoryHandle` is now a local-memory handle, not a vector database handle. The
canonical workspace path is:

```text
<workspace>/.rara/memory
```

Full records are stored in:

```text
<workspace>/.rara/memories/records.json
```

Session checkpoints are stored in:

```text
<workspace>/.rara/rollouts/<session-id>/context.jsonl
```

For backward-compatible serialized shapes, session checkpoints may still carry
an empty `vector` field, but the runtime does not produce or consume vectors.

## Contracts

- Local memory writes must not require an embedding backend.
- Local memory search must not call the LLM backend.
- Search ranking is text-based and should stay explainable through
  `MemorySelection` selected/dropped reasons.
- Auto-memory extraction may call the active LLM backend to distill facts, but
  persistence and retrieval remain local file operations.
- Config fields for old local embedding policy are compatibility-only and must
  not start bundled sidecars.
- TUI/status surfaces should report local semantic memory as disabled and point
  future semantic recall to official Mem.

## Validation Matrix

| Contract | Validation |
|----------|------------|
| No embedded vector dependencies | `rg "lancedb|LanceDB|VectorDB|vectordb|vector_memory" src crates/rara-memory crates/rara-tools Cargo.toml crates/*/Cargo.toml` |
| No embedding trait/runtime path | `rg "EmbeddingBackend|EmbeddingInputKind|async fn embed|hashed_embedding" src crates/rara-memory crates/rara-tools` |
| Local records search without LLM calls | `cargo test memory_store -- --nocapture` |
| Session shard recall remains text-based | `cargo test session_context -- --nocapture` |
| Retrieval candidates still assemble correctly | `cargo test context::assembler -- --nocapture` |
| Production code typechecks | `cargo check` |

## Operational Notes

- Existing LanceDB directories are no longer consulted by the runtime.
- Historical journals may still describe the old LanceDB/vector implementation;
  this spec is the current contract.
- If semantic search is required, integrate through official Mem rather than
  reintroducing a local vector backend.

## Source Journals

- [2026-08-08-remove-embedded-vector-memory.md](../journal/2026-08-08-remove-embedded-vector-memory.md)
