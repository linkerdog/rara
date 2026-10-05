# Threads

## Problem

Sessions saved as flat JSON by `SessionManager` — no browse, search, pin,
export, or distillation surface. Cannot recall past conversations or distill
durable memories.

## Scope

`Thread` is a durable conversation record. Threads are the raw material from
which `MemoryRecord`s are distilled.

Threads are not memories. A thread can be long, contextual, and useful only when
read with its surrounding messages. A memory must stand on its own as one
durable decision, insight, fact, procedure, or experience.

## Data Model

```rust
pub struct Thread {
    pub id: Uuid,
    pub title: String,
    pub source: ThreadSource,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub messages: Vec<ThreadMessage>,
    pub distilled_memory_ids: Vec<Uuid>,
    pub pinned: bool,
}

pub enum ThreadSource {
    RaraSession { session_id: String },
    CodexImport { source_session_id: String },
    FileImport { path: String, format: ImportFormat },
    ManualCapture,
}
```

## Lifecycle

```
Session Active
  ├─ stop hook → save_thread(auto)
  ├─ /save → save_thread(manual)
  ├─ /save-handoff → summary only
  ▼
Thread stored (LanceDB)
  ├─ /distill → MemoryRecords
  ├─ /export → Conversation Markdown
  ├─ /threads → browse, search, pin
```

## Save Triggers

| Trigger | Behavior |
|---------|----------|
| Stop hook | Auto-save after each response |
| Session end | Final turn batch on clean exit |
| `/save` | Explicit full-session save |
| `/save-handoff` | Concise continuation summary (not full thread) |

## ThreadStore API (LanceDB)

- `save(thread) -> Uuid`
- `get(id) -> Option<Thread>`
- `list(pinned_only?, source_filter?, limit, offset) -> Vec<ThreadSummary>`
- `search(query, limit) -> Vec<ThreadSummary>`
- `search_messages(thread_id, query) -> Vec<ThreadMessage>`
- `pin(id, pinned) -> ()`
- `delete(id) -> ()`

Storage: `~/.rara/threads/`.

Current backend slice:

- `load_thread(session_id) -> ThreadSnapshot` materializes canonical history,
  structured thread metadata, plan state, interactions, turns, and compaction events.
- `ThreadStore` can be constructed from explicit rollout and legacy-session
  roots; the materialization path reads `transcript.jsonl`, `history.json`,
  legacy history, and compaction migrations directly instead of delegating those
  reads back through `SessionManager`.
- History checkpoints enter through `ThreadRecorder`, update the canonical
  `transcript.jsonl` before writing the compatibility `history.json` snapshot,
  and therefore keep resume from observing a newer snapshot ahead of the typed
  transcript.
- `fork_thread(session_id) -> String` copies the materialized state into a new
  session id and records lineage.
- `ThreadRecorder` owns append/flush/shutdown operations for structured rollout
  items and routes compaction events through the same append-only recorder path.
- Runtime rollout snapshots are normalized by `ThreadRecorder` into one
  canonical `runtime_state` event before appending to `events.jsonl`; `StateDb`
  side tables remain compatibility/index surfaces.
- Runtime metadata is written to per-session `thread.json` before updating the
  `StateDb` listing/index row. `ThreadStore` prefers `thread.json` and only
  falls back to `StateDb` metadata for older sessions.
- An optional explicit `title` is stored in canonical metadata and the SQLite
  index. Metadata/index mutations share a per-thread advisory lock; ordinary
  checkpoints preserve the stored title. Named empty threads are resumable.
  Older records and databases gain the optional title without rewriting history.
- `ThreadStore` treats `runtime_state` entries as snapshots and materializes
  only the latest snapshot into current plan/interactions, avoiding duplicate
  rollout items from stale snapshots.
- Committed TUI turns are appended to per-session `turns.jsonl` through
  `ThreadRecorder`; `ThreadStore` prefers that log and only falls back to
  `StateDb` turn rows for older sessions.
- In-progress TUI turns are written entry-by-entry to per-session `live.jsonl`.
  The TUI clears that live log immediately after committing a full turn to
  `turns.jsonl`; resume loads any remaining live entries back into the active
  turn so an interrupted process can recover partial transcript output without
  treating it as a committed turn.
- Live-log recovery preserves valid entries around malformed records and reports
  incomplete recovery to the caller. Missing logs are normal; open/read errors
  and skipped records produce a visible recovery warning. Diagnostics never
  include raw corrupt transcript contents or write directly to the terminal.
- A failed StateDb turn index update after a successful canonical append remains
  a recoverable indexing failure, not a failed canonical commit. Report it as a
  warning without retrying or duplicating the committed turn.
- Runtime compaction writes also go through `ThreadRecorder`, so manual/auto
  compaction and fork replay share the same structured rollout event boundary.
- `export_thread_markdown(session_id) -> String` renders a portable markdown
  transcript with frontmatter, summary, and message sections.
- Export prefers committed display turns, which retain pre-compaction content
  and turns hidden by `/clear`; legacy threads fall back to message history.
  Display clearing never resets the durable turn ordinal. Source corruption
  fails export explicitly. The TUI also supports versioned JSON and atomic,
  no-overwrite file creation as specified in [session commands](session-commands.md).
- `distill_thread_summary(memory_store, session_id) -> Option<MemoryRecord>`
  persists one summary-style `MemoryRecord` linked to the source session/thread.

The current implementation still keeps `history.json` as a compatibility
snapshot beside the canonical transcript and keeps `StateDb` as the listing and
legacy side-table fallback. The structured thread source lives under the
per-session rollout directory rather than a dedicated LanceDB thread table.

## Indexed Thread Listing

The SQLite thread index filters resumable sessions before applying a page limit.
Search is a literal substring over the latest turn preview, session ID, full
cwd, branch, provider, model, execution mode, and approval mode. ASCII case is
ignored; Unicode text is matched literally. This is metadata/preview search,
not full-history search. Percent signs and underscores are ordinary characters.

Queries accept an exact stored cwd, an excluded current session, and updated or
created ordering. Descending `(timestamp, session_id)` cursors provide stable
pagination when timestamps tie. Pages preserve the existing resumability
predicate and return errors rather than an empty success. The existing recent
thread APIs retain their behavior. No schema or persistent format changes are
required.

The TUI executes indexed queries through its ordered storage owner. New search,
scope, sort, or source-session inputs invalidate pending results; appended pages
are deduplicated by session ID. This is a live listing, so updates can move
sessions across a page boundary until refresh restarts the query.

Verification covers old matches outside the newest 200 records, literal search
characters, exact cwd/current-session exclusion, tied timestamps in both sort
orders, and asynchronous scope/search/source changes. See the
[implementation checkpoint](../journal/2026-10-05-resume-indexed-search.md) and
[resume interaction contract](../interaction/composer-and-overlays.md#resume-search-and-scope).

## Conversation Markdown Format

```markdown
---
title: Python Async Patterns
source: rara
date: 2026-05-03
---

## User
How does async/await work?

## Assistant
Python's async/await lets you write concurrent code...

## Tool
read_file path="src/main.rs"
```

Headers: `## User`, `## Assistant`, `## System`, `## Tool`, `## Tool Result`.
Optional YAML frontmatter with `title`, `source`, `date`.

## Import Paths

| Source | Format |
|--------|--------|
| Conversation Markdown | `.md` with `## User`/`## Assistant` |
| ChatGPT | `chat.html` |
| Claude | `conversations.json` |
| DeepSeek | `deepseek_conversations.json` |

## Thread Distillation

`MemoryDistiller`: Thread → 2-8 MemoryRecords.
- Read full thread messages.
- Identify independently meaningful units.
- Auto-generate title, labels, importance.
- For >50 messages: chunked Smart Background Distillation.
- Treat older memories and earlier conclusions as historical context, not
  current truth. If the thread proves that a prior memory is stale, incomplete,
  or tied to a poor current design, distillation should capture the corrected
  durable fact or procedure.
- It is valid to distill a durable tooling need when a thread establishes that
  reliable future work needs a small purpose-built tool or runtime hook.

Distillation rules:

- Do not persist every message as a memory.
- Prefer fewer, independently useful records over many thin summaries.
- Preserve source provenance: thread id, message span, session id, and
  workspace scope.
- Deduplicate against existing memories before insert.
- Use `MemorySelection` for any immediate context carry-over; do not inject
  distilled memories directly into prompts.

Runtime status:

- Implemented: summary distillation for a loaded thread into one
  `ThreadDistill` memory record with `session_id`, `thread_id`, and source span.
- Implemented: LLM-assisted extraction of 2-8 independently useful records
  through `ThreadStore::distill_thread_memories`, with duplicate detection
  against same-batch drafts and existing memory search hits.
- Open: long-thread chunking, finer per-memory source spans, and background
  promotion scheduling.

## Source Journals

- [Runtime diagnostics and recovery](../journal/2026-10-05-runtime-diagnostics.md)
- 2026-05-03-memory-records-and-threads-spec.md
- 2026-05-03-memory-record-persistence-and-thread-export.md
