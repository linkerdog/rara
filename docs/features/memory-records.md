# Memory Records and Storage

## Problem

RARA needs structured, agent-authored memory. The mock `VectorDB` returns empty
results, so retrieval is limited to tool-result extraction from conversation
history. Without real memory records, each session starts fresh.

## Scope

`MemoryRecord` is the durable, independently meaningful unit of memory — one
decision, insight, fact, procedure, or experience. The storage path uses
LanceDB as a unified local memory index: raw text, metadata, full-text search,
and vector search live in one table, while context assembly still goes through
`MemorySelection`.

This spec describes the target product contract. The current implementation
slices provide the LanceDB-backed index, retrieval tools, a runtime
`MemoryStore` facade, ranked `MemorySelection` candidates, pinned retention
metadata, update/delete/list-label scaffolding, LLM-assisted thread
distillation into multiple durable records, and an explicit session-shard
promotion API. Periodic scheduling and richer filtering remain follow-up work.

## Six Design Laws (Cross-Industry Consensus)

### Law 1: Non-Derivable Principle
Don't persist what can be retrieved live. Stale memory > no memory.

### Law 2: Human Memory Priority
`memory.md` unconditional; AI memories compete for budget.

### Law 3: Multi-Layer Isolation
User scope (`~/.rara/memories/`), project scope (`memory.md`), session scope.

### Law 4: Path as Primary Signal
`MemoryStore::search` accepts `scope_path`; local results rank above global.

### Law 5: Human Memories Immune to Forgetting
`UserCreated` records exempt from automatic cleanup.

### Law 6: Negative Space First
`create_memory` prompt includes what-NOT-to-save section.

## RARA Positioning (Five Axes)

| Axis | Position |
|------|----------|
| Extraction | Automation-first (novice users), human override available |
| Storage | LanceDB structured with FTS + vector columns, `memory.md` stays flat-file |
| Injection | Zero-call for human sources, budgeted hybrid retrieval for AI |
| Forgetting | Discrete with importance gating; `UserCreated` exempt |
| Architecture | Core built-in now, plugin surface deferred |

## Data Model

```rust
pub struct MemoryRecord {
    pub id: Uuid,
    pub title: String,
    pub content: String,       // Markdown
    pub labels: Vec<MemoryLabel>,
    pub importance: f32,       // 0.1–1.0
    pub pinned: bool,
    pub source: MemorySource,
    pub created_at: DateTime,
    pub embedding: Option<Vec<f32>>,
}

pub enum MemoryLabel { Insight, Decision, Fact, Procedure, Experience }
pub enum MemorySource { AgentTurn, UserCreated, ThreadDistill, FileImport }
```

## Product Contract

RARA memory should eventually behave like a durable knowledge object, not just a
retrieval row.

Each memory owns:

- `title`: short human-readable summary.
- `content`: Markdown body containing the durable knowledge.
- `labels`: reusable classification tags for filtering and routing.
- `importance`: ranking signal from `0.1` to `1.0`.
- `pinned`: explicit retention guard for durable facts that must not be removed
  by automatic cleanup.
- `created_at` and `updated_at`: temporal search and evolution metadata.
- `source`: provenance such as user-created, agent turn, thread distillation,
  file import, or protocol write.
- `scope`: user, workspace, project, thread, or session visibility boundary.
- `session_id`, `thread_id`, and `source_span`: optional provenance linking the
  memory back to the session/thread and turn range that produced it.
- `embedding`: optional vector representation for semantic retrieval.

Standard labels:

| Label | Intended Use |
|-------|--------------|
| `insight` | Durable lessons and realizations. |
| `decision` | Choices with rationale and trade-offs. |
| `fact` | Reference information and stable data points. |
| `procedure` | Repeatable workflows and steps. |
| `experience` | Events, conversations, outcomes, and incident notes. |

Importance scale:

| Range | Meaning |
|-------|---------|
| `0.8..=1.0` | Critical architectural decisions, incidents, or high-value procedures. |
| `0.5..0.8` | Useful project learnings and ordinary decisions. |
| `0.1..0.5` | Background reference and low-priority notes. |

## Product Capability Matrix

| Capability | Target Behavior | Current Runtime Status |
|------------|-----------------|------------------------|
| Memory record anatomy | Title, Markdown content, labels, importance, pinned status, timestamps, source, scope, embedding, and provenance. | Partial. `MemoryRecord` is now persisted as the domain record; LanceDB rows still store the compact search index shape. |
| Memory creation | Agent or user creates a durable `MemoryRecord`; title, labels, and importance can be generated or explicit. | Partial. `remember_experience` is now a compatibility adapter over `MemoryStore::insert`. |
| Memory search | Hybrid semantic + keyword search with metadata filters and explainable scores. | Partial. LanceDB vector, FTS, and hybrid helpers exist; `MemoryStore::search` rehydrates full persisted records before returning hits. |
| Memory update | Existing records can be edited without creating duplicates. | Partial. `MemoryStore::update` updates domain records and refreshes the LanceDB row when content changes; `MemoryControlHandler` exposes this through structured control-plane requests. |
| Memory delete | User or control-plane request can delete records with audit-safe semantics. | Partial. `MemoryStore::delete` removes the domain record and search rehydration filters stale indexed rows; `MemoryControlHandler` exposes deletion through structured control-plane requests; physical LanceDB row cleanup remains future work. |
| Memory retention | Pinned, user-created, and high-importance memories are protected from automatic cleanup; explicit delete remains possible with provenance. | Implemented as a domain guard on `MemoryRecord`; no automatic cleanup path exists yet. |
| Thread distillation | Thread history can be distilled into 2-8 durable memory records. | Implemented for loaded threads through `ThreadStore::distill_thread_memories`, with LLM-assisted extraction, batch/existing-memory deduplication, and thread provenance. Long-thread chunking remains future work. |
| Session-shard promotion | Session context shards can be promoted into durable memory records without writing raw checkpoints to the global index by default. | Partial. `SessionManager::promote_session_context_memories` explicitly distills selected shard checkpoints into `MemoryRecord`s with session provenance; periodic scheduling remains future work. |
| Context injection | Ranked memory candidates pass through `MemorySelection` before prompt injection. | Partial. LanceDB-backed memory and session search now produce direct ranked `MemorySelection` candidates; retention, deduplication, and protocol mutation remain future work. |
| Graph retrieval | Entity and relationship traversal complements vector recall. | Future work. |
| Working memory | Daily or session briefing summarizes recent and important memories. | Future work. |
| MCP / ACP / Wire memory APIs | Protocol clients can query and mutate memory through the runtime control plane. | Partial. Runtime-control requests can add, update, delete, list labels, query metadata, and query records through `MemoryControlHandler`; transport-specific command surfaces remain follow-up work. |

## Memories vs Threads

Threads preserve conversation history. Memories preserve durable knowledge.

RARA should not treat every thread message as a memory. Raw turn checkpoints are
useful for crash recovery, browsing, and future distillation, but a
`MemoryRecord` must be independently useful without the full thread.

The runtime should therefore keep three separate objects:

- `Thread`: full or summarized conversation record.
- `MemoryRecord`: distilled durable knowledge unit.
- `MemorySelectionItem`: per-turn context candidate selected from prompt files,
  thread recall, memory retrieval, or future protocol sources.

This separation prevents a storage backend from bypassing context policy:
LanceDB may store and retrieve candidates, but `MemorySelection` decides whether
they enter the model context.

## Session Shards and Global Memory

Session-level history should stay local to the session. The target storage
shape is one append-oriented shard per session, so active agent turns can write
without contending on the global memory index. A shard may be a LanceDB table,
state-db artifact, or another append-friendly file format, but it must be
addressed by session id and remain cheap to restore, compact, or delete.

Global memory has a different contract. It should contain durable
`MemoryRecord`s promoted from explicit memory tools, user actions, protocol
writes, and periodic distillation of session shards. Raw turn checkpoints should
not be written to the global memory index by default. Promotion into global
memory should be scheduled or batched so cross-session recall stays useful
without turning every active turn into a global write.

The former `conversations` LanceDB table was an interim checkpoint path. Raw
session-context checkpoints now write to per-session append shards under the
rollout directory instead of the global memory index.

## Scope Promotion Rules

Promotion turns transient conversation or protocol input into durable
`MemoryRecord`s. The record `scope` is the storage and index boundary. Provenance
fields such as `session_id`, `thread_id`, and `source_span` explain where the
memory came from, but they must not override the selected scope.

The runtime promotion rules are:

- `Workspace` promotion stores the record in the workspace bucket. It may keep
  `session_id` and `thread_id` as provenance, but the LanceDB index key remains
  `workspace`.
- `Thread` promotion requires a non-empty `thread_id`. `session_id` is optional
  provenance. The LanceDB index key is the `thread_id`.
- `Session` promotion requires a non-empty `session_id`. It mirrors that id into
  `thread_id` for compatibility with current session-thread provenance. The
  LanceDB index key is the `session_id`.

Protocol memory writes follow the same rules. A protocol client may add
workspace-scoped memory without a thread id, but thread-scoped memory must pass
`thread_id` in metadata so future control-plane clients cannot create ambiguous
thread records.

## Periodic Promotion Policy Gate

Periodic session-shard promotion must be opt-in. The explicit API
`SessionManager::promote_session_context_memories` remains available for manual
or control-plane-triggered promotion, but scheduler-style callers should use the
policy-gated path:

- `SessionShardPromotionPolicy` defaults to disabled.
- `min_checkpoints` avoids promoting tiny or accidental shards.
- `max_checkpoints` bounds the tail of context checkpoints passed to
  distillation.
- `SessionShardPromotionTrigger` records whether the attempt came from a
  periodic scheduler, shutdown hook, or runtime-control request.
- `SessionShardPromotionOutcome` reports eligible/skipped state and promoted
  count so `/context`, ACP/Wire, and future OTEL exporters can observe promotion
  attempts without parsing logs.

The first gated path does not install a timer. It provides the contract that any
future scheduler must call before writing durable memory in the background.

## MemoryStore API

- `insert(record) -> MemoryRecord` — persist with auto-embedding
- `search(query, labels?, min_importance, scope?, limit) -> Vec<(MemoryRecord, f32)>`
- `update(id, patch) -> MemoryRecord`
- `get(id) -> Option<MemoryRecord>`
- `delete(id) -> ()`
- `set_pinned(id, pinned) -> MemoryRecord`
- `list_labels(scope?) -> Vec<(MemoryLabel, usize)>`

Search ranking should not rely on LanceDB score alone. The final memory ranking
layer should combine hybrid search score, `importance`, exact keyword/path
matches, recency where appropriate, and duplicate suppression. `/context` should
show the selected/dropped reason for memory candidates, including whether
`importance` or `pinned` status affected the decision.

Storage:

- `~/.rara/memories/records.json`: durable `MemoryRecord` domain records.
- `~/.rara/lancedb/`: compact search index with text, vector, and source keys.

Local write coordination: RARA uses an adjacent advisory lock file
(`~/.rara/lancedb.lock`) for LanceDB mutations. Reads remain lock-free, while
table creation, index creation, upsert, and future update/delete paths must
serialize through this lock so multiple RARA processes can share the same
workspace memory directory without racing initialization or commits. Domain
record writes use their own adjacent lock file next to `records.json`, so the
record truth and the search index can evolve independently without depending on
LanceDB schema migrations.

## LanceDB Index Contract

The first runtime slice keeps the existing `VectorDB` façade but backs it with
LanceDB instead of a mock.

The current table shape is intentionally small:

- `id`: stable memory id.
- `session_id`: source session or scope.
- `turn_index`: source turn index or deterministic tool-write id.
- `text`: raw memory text; indexed with LanceDB FTS / BM25.
- `vector`: embedding column; searched with LanceDB vector search.

Search modes:

- vector search via `search_with_metadata`;
- FTS search via `full_text_search_with_metadata`;
- hybrid search via `hybrid_search_with_metadata`, combining LanceDB FTS and
  vector search while returning debug scores (`fts_score`, `vector_distance`).

Search must not create tables. Only write paths create tables using the real
embedding dimension. This avoids fixing an empty table to a guessed vector
dimension before the first memory write.

## Integration

| Component | Integration |
|-----------|-------------|
| `remember_experience` | Current compatibility tool; should become a thin adapter over `MemoryStore::insert` |
| `memory_add` / `memory_update` / `memory_delete` | Future protocol-safe memory mutation tools |
| `retrieve_experience` | Current compatibility retrieval tool; should delegate to `MemoryStore::search` |
| `memory_search` | Future protocol-safe search tool with labels, scope, and importance filters |
| `MemorySelection` | `vector_memory_candidate` becomes `selectable: true` |
| `MemoryDistiller` | Thread → MemoryRecords with auto-labels + importance |

Current implementation checkpoint:

- `MemoryStore` owns the memory-domain runtime facade over the LanceDB-backed
  index.
- `MemoryStore` persists full domain records in `records.json`; search uses
  LanceDB for recall and then rehydrates the full record by id.
- `MemoryStore::set_pinned` updates persistent retention metadata without
  touching the LanceDB search row.
- `MemoryStore::update`, `delete`, and `list_labels` provide the memory-domain
  API needed by future ACP/Wire adapters without exposing LanceDB operations.
- Protocol memory control requests now execute add, update, delete, list-label,
  metadata, and record-query operations through `MemoryStore` and publish
  structured memory events. Protocol adapters still must route their
  transport-specific commands through the shared control-plane dispatcher.
- Search rehydration treats persisted `MemoryRecord`s as the source of truth:
  indexed rows with deleted ids are filtered instead of reconstructed.
- `MemoryRecord::is_protected_from_automatic_cleanup` protects pinned,
  user-created, and high-importance records from future automatic cleanup paths.
- `remember_experience` writes through `MemoryStore::insert`.
- `retrieve_experience` searches through `MemoryStore::search` and returns both
  `relevant_experiences` and memory diagnostics.
- `retrieve_session_context` searches per-session context shards instead of
  returning a stub response.
- `ThreadStore::distill_thread_summary` remains as the compatibility path for
  promoting one summary-style record.
- `ThreadStore::distill_thread_memories` promotes a loaded thread into 2-8
  independently useful `MemoryRecord`s using `MemoryDistiller`.
- `SessionManager::promote_session_context_memories` explicitly promotes
  selected per-session context checkpoints into durable `MemoryRecord`s using
  the same `MemoryDistiller` and duplicate suppression path as thread
  distillation. The runtime does not schedule background promotion yet.
- The distillation prompt treats older memory and prior conclusions as
  historical context, not current truth. If the inspected thread proves the
  current design is stale or poor, the distiller should extract the corrected
  durable fact, procedure, or need for a small purpose-built tool instead of
  preserving the old state.
- Generated records preserve `session_id`, `thread_id`, and a source span over
  the source thread. The first implementation uses the whole loaded thread as
  the span; finer per-message spans remain future work.
- Agent turn checkpoints write to per-session `context.jsonl` shards under
  `rollouts/<session_id>/`.
- `MemoryRetrievalOrchestrator` promotes LanceDB-backed workspace memories and
  per-session context hits into direct ranked `MemorySelection` candidates.
- Selected retrieval candidates are rendered as a per-turn internal context
  block prepended to the current user request. They are not persisted into
  thread history and do not change the stable system prompt prefix.

## Migration

1. Replace mock `VectorDB` with LanceDB-backed FTS/vector/hybrid index.
2. Wire retrieval tools to the LanceDB-backed index.
3. Add the `MemoryRecord` domain model with title, labels, importance, source,
   scope, and timestamps. Done.
4. Add the `MemoryStore` domain façade over `VectorDB`. Done.
5. Make `remember_experience` and `retrieve_experience` compatibility adapters
   over `MemoryStore`. Done.
6. Persist full `MemoryRecord` records separately from the LanceDB search index.
   Done.
7. Wire `MemorySelection` to ranked memory candidates. Done.
8. Add pinned/retention policy so pinned, user-created, and high-importance
   memories are excluded from automatic cleanup. Done.
9. Add update/delete/list-label control-plane scaffolding without exposing
   storage internals. Done.
10. Add thread distillation into `MemoryRecord`. Done for loaded threads:
    summary distillation remains as compatibility, and LLM-assisted multi-record
    extraction with deduplication is available through `distill_thread_memories`.
11. Move raw session checkpoints out of the global `conversations` LanceDB table
   into per-session append shards. Done.
12. Add explicit promotion from session shards into global `MemoryRecord`s.
    Done for API-triggered promotion.
13. Add scheduler/policy gates for periodic session-shard promotion.
    Done for opt-in policy evaluation and observable outcomes; a real periodic
    timer remains future work.
14. Deprecate `VectorDB`.
15. Remove `VectorDB`.

## Source Journals

- 2026-05-03-memory-records-and-threads-spec.md
- 2026-05-03-lancedb-memory-index.md
- 2026-05-03-memory-record-persistence-and-thread-export.md
