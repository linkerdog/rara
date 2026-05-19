# Memory Subsystem

## Overview

RARA's memory system stores structured records (via LanceDB + file persistence)
and unstructured memory files (`summary.md`, `MEMORY.md`, session files) under
`~/.rara/memory/`.

Inspired by Claude Code's flat-file model and Codex's two-phase write path.

## Architecture

```
Agent turn
    │
    ▼
context/assembler.rs ─── reads summary.md + hooks ──→ system prompt
    │
    ▼
agent/memory_retrieval.rs ─── search/retrieve ──→ MemoryStore
    │
    ▼
memory_selection.rs ─── selects candidates ──→ system prompt
    │
    ▼
(distilling) memory_distiller.rs ─── LLM extracts memory ──→ MemoryStore::insert()
    │
    ▼
memory_store_impl.rs ─── CRUD ──→ PersistedMemoryRecordFile + LanceDB

```

## Key Types

- `MemoryRecord` — core record with id, source (project/thread/user), scope, labels, tags, content, metadata
- `NewMemoryRecord` — input for `MemoryStore::insert()`
- `MemoryRecordSearchHit` — search result with snippet, score, highlight
- `MemoryLabel` — categorization: `Decision`, `Context`, `KnownIssue`, `Pattern`, `Preference`
- `MemorySource` — where it came from: `Distilled(thread_id)`, `Manual`, `Imported`, `File`
- `MemoryRecordPatch` — partial update fields
- `MemoryDistiller` — takes thread_markdown, returns `Vec<DistilledMemoryDraft>`

## Storage Layers

1. **LanceDB** (`crates/rara-memory/src/vectordb.rs`) — vector search via embedding
2. **File persistence** (`memory_records.rs`) — JSON files, one per record
3. **Memory files** (`crates/rara-memory/src/files.rs`) — `summary.md`, `MEMORY.md`, session files

## Concurrent Safety

- `summary.md` writes: `fs2` exclusive lock via `with_file_lock()`
- Per-file writes: `atomic_write()` (temp file + rename)
- LanceDB writes: thread-safe via internal locking

## File Map

| File | Purpose |
|---|---|
| `memory_store.rs` | Facade including all sub-modules |
| `memory_types.rs` | All type definitions |
| `memory_store_impl.rs` | MemoryStore CRUD |
| `memory_records.rs` | File persistence layer |
| `memory_store_helpers.rs` | Utilities |
| `memory_distiller.rs` | LLM-powered distillation |
| `memory_files.rs` | Re-export shim for crate |
| `memory_notice.rs` | In-context notice formatting |
| `memory_selection.rs` | Context memory selection |
| `auto_memory.rs` | Automatic memory extraction trigger |

## Known Gaps

- MemoryStore depends on `crate::llm` (LlmBackend) — blocks migration to `rara-memory` crate
- No two-phase write path (Phase1 ad-hoc → Phase2 consolidation)
- LanceDB search returns placeholder empty (not wired to real embedding backend)
- No memory hooks (pre/post write events)
