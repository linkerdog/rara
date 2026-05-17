# Session and Global File-Based Memory with Summary-Driven Retrieval

## Problem

RARA currently has placeholder vector retrieval (`RetrieveMemory` returns empty)
and relies on conversation-history extraction. No durable, session-scoped
memory exists across restarts.

Codex stores durable memories in `~/.codex/memory.md` and project-level
preferences in `AGENTS.md`. Claude uses `CLAUDE.md`. Both inject the full file
into context — simple, reliable, zero-search.

RARA needs a similar flat-file foundation, enhanced with a **memory summary**
that guides retrieval: the summary knows where useful information lives (which
session file, which LanceDB collection) and helps the agent decide whether to
search files or query the vector store.

## Scope

- Session-scoped memory files under `~/.rara/memory/sessions/<session-id>.md`
- Global-scoped memory file at `~/.rara/memory/global.md`
- A memory summary file at `~/.rara/memory/summary.md` that indexes and
  routes to relevant sources
- Runtime tools to read, write, and search these files
- Integration with existing `MemoryRecord` / LanceDB pipeline
- TUI auto-memory extraction targets both session files and LanceDB

## Non-Goals

- Replacing LanceDB (it remains the vector store for semantic search)
- File-based full-text search engine (LanceDB FTS handles that)
- Changing the `MemoryRecord` data model

---

## Design

### 1. File Layout

```
~/.rara/memory/
  global.md          ← durable cross-session memory
  sessions/
    <session-id>.md  ← per-session memory (session boundary)
  summary.md         ← index of what lives where
```

#### `global.md` format

```markdown
# RARA Memory

## Preferences
- Prefer `uv` over system Python for venv creation
- Use short commit titles, imperative mood

## Decisions
- Chose `include!` over submodules for local_model_server split (2025-07-15)
  Reason: Rust glob re-exports don't propagate `pub(crate)` to sibling modules

## Lessons
- `python --version` writes to stderr, not stdout — check_output misled us
```

#### Session file format

```markdown
# Session abc123 — 2025-07-15

## Key Findings
- `check_python_version` returned `Ok(())` for missing binaries — root cause of venv failures

## Decisions Made
- Switched from `find_python310_plus` to `uv venv --python 3.14`
- Replaced `RENDERABLE_SYSTEM_MESSAGE_PREFIXES` with `SystemMessageKind` enum

## Files Touched
- src/local_model_server.rs → split into 10 include! files
- src/tui/render/cells/mod.rs → removed prefix list
- docs/todo.md → recorded tui/state split limitation
```

#### `summary.md` format

```markdown
# Memory Summary

## Global
- preferences: uv over system Python, commit conventions
- decisions: include! split pattern, SystemMessageKind enum
- lessons: python --version → stderr, RENDERABLE_SYSTEM_MESSAGE_PREFIXES brittle

## Session abc123 (2025-07-15)
- venv creation fix (uv, --seed, --python 3.14)
- local_model_server file split
- SystemMessageKind refactor

## Session def456 (2025-07-14)
- tui/state/mod.rs split investigation (concluded: refactor impl block first)

## LanceDB Collections
- memory_main: 42 records (decisions, facts, procedures)
- memory_embedding: 384-dim all-MiniLM-L6-v2 vectors
```

### 2. Memory Summary as Retrieval Router

The summary is the **first thing loaded** into context when memory retrieval is
needed. It tells the agent:

| Question | Answer from summary |
|----------|---------------------|
| "What do I know?" | Lists all major topics |
| "Where is it?" | Points to `global.md`, session file, or LanceDB |
| "How to find more?" | LanceDB FTS for keyword, vector search for similarity |

**Flow:**

```
User asks: "How did we fix the venv issue?"

1. Load summary.md into context (~1KB)
2. Summary says: "venv creation fix → session abc123"
3. Agent reads ~/.rara/memory/sessions/abc123.md
4. If more detail needed → LanceDB FTS: "venv uv python"
```

This avoids loading ALL session files into context every turn.

### 3. Memory Extraction

**Auto-memory (background TUI task):**

Every 5 agent turns (matching the existing `EXTRACTION_INTERVAL`), the TUI
memory extractor runs. It writes to:

1. `<session-id>.md` — appends to the current session file
2. LanceDB — creates `MemoryRecord`(s) via `MemoryStore`
3. `summary.md` — updates the index with new topics

**On session end:**

- The session file is finalized
- `summary.md` is updated with a concise session summary
- Long-running threads may promote session findings to `global.md`

### 4. Runtime Tools

| Tool | Reads/Writes | Purpose |
|------|-------------|---------|
| `read_memory_file` | `global.md`, session files | Read memory file (path sanitized) |
| `write_memory` | `global.md` (append), session file (append) | Agent-authored memory |
| `search_memory` | All files + LanceDB | Unified search (file grep + LanceDB FTS) |
| `memory_summary` | `summary.md` | Load the routing index |
| `promote_to_global` | `global.md` | Promote session finding to durable memory |

`search_memory` delegates:
- Keyword search → `rg` on `~/.rara/memory/` files; falls back to native
  `std::fs::read_dir` + `String::contains` loop if `rg` is not installed
- Semantic search → LanceDB vector query
- Combined: merge results from both sources, deduplicate by content hash

### 5. Retention Policy

**summary.md** grows with each session. To prevent context-window exhaustion:
- Cap summary.md at ~5KB (roughly 50 session entries)
- When the cap is hit, condense the oldest session entries into a single
  "Archived sessions" line: `## Sessions abc123..def456 (2025-07-01..2025-07-15)`
  with just the count and date range — detail stays in individual session files
- Old session files are never deleted; only the summary index is condensed

### 6. Security

`read_memory_file` and `write_memory` MUST validate that the target path
resolves within `~/.rara/memory/`. Reject any path containing `..` or
symlink traversal outside the memory root.

### 7. Context Injection

On each turn, memory injection follows this priority:

1. **`summary.md`** — always loaded (small, ~1KB, capped by retention policy)
2. **`global.md`** — loaded when context budget allows
3. **Relevant session files** — loaded on-demand via `search_memory` / `read_memory_file`
4. **LanceDB records** — loaded via `search_memory` when vector search is needed

`project_memory` (`AGENTS.md`) continues to be injected per existing rules.

### 8. Integration Points

| Existing Component | Change |
|--------------------|--------|
| `MemoryStore` | Add file-based backend alongside LanceDB |
| `MemorySelection` | Include summary-guided routing decisions |
| `context/assembler` | Load `summary.md` into system context |
| TUI memory extractor | Write to session files + LanceDB |
| `/memory` command | New TUI command surface for memory inspection |

### 9. TUI Display

```
# Context
...
── Memory ──
  summary:  loaded (12 topics, 3 sessions)
  global:   24 entries (loaded)
  session:  abc123.md (6 entries, loaded)
  lancedb:  42 records (via memory_main)
── End Memory ──
```

---

## Implementation Plan

### Phase 1: File I/O Foundation (~100 lines)
- Create `~/.rara/memory/` directory structure on startup
- `read_memory_file(path)` with path-traversal protection
- `write_memory(scope, content)` function
- Session file auto-created on first write

### Phase 2: Summary Index (~80 lines)
- `summary.md` format and parsing
- Auto-update on session file writes
- Retention policy: condense old entries when >5KB
- `memory_summary` tool

### Phase 3: Unified Search (~60 lines)
- `search_memory(query)` — delegates to `rg` (with native fallback) + LanceDB
- Merge results from both sources, deduplicate

### Phase 4: Context Integration (~40 lines)
- Inject `summary.md` into system context
- Wire `read_memory_file` and `search_memory` as agent tools

### Phase 5: TUI Commands (~50 lines)
- `/memory` command to inspect state
- Auto-memory extractor writes to session files (every 5 turns)

---

## Verification

- Create a session, write memories, verify file exists
- Run `search_memory "venv"` — returns merged results (file + LanceDB)
- Restart RARA — session file still present, `summary.md` loaded into context
- Promote a session finding to `global.md` — verify it persists
- Send path-traversal attempt to `read_memory_file` — verify rejection
- Verify `rg` not installed → native fallback still returns results
