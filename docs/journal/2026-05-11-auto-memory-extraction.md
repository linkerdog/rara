# 2026-05-11-auto-memory-extraction

Implemented two P0 features from Claude Code / Codex patterns.

## Auto-memory extraction (`src/auto_memory.rs`)

After every 5 completed turns, collects recent user/assistant messages
and uses the LLM backend to extract durable facts. Results are inserted
into LanceDB via MemoryStore with `AutoMemory` provenance.

### Design decisions

- **Debounce**: triggers every 5 turns to avoid excessive LLM calls
- **Prompt template**: aligned with Codex's stage_one extraction prompt
  ("Extract durable facts from this conversation…")
- **Background execute**: spawned via `tokio::spawn`, does not block
  the TUI thread
- **State tracking**: uses `committed_turns.len()` as turn counter proxy
  (avoids adding a dedicated field to TuiApp)

### Integration point

`tasks.rs` AgentStop handler → `maybe_auto_memory(app, agent)` after
`finalize_active_turn()`.

## Directory-walking rules layer (`crates/instructions/src/workspace.rs`)

Walks `.rara/rules/*.md` files from CWD up to repo root, injecting them
as `ProjectInstruction` prompt sources. Enables per-directory rules in
monorepo setups.

### Implementation

- `RULES_DIR_NAME = ".rara/rules"` — scanned in each ancestor directory
  alongside existing `AGENTS.md` / `CLAUDE.md` discovery
- Files with `.md` extension collected, cached via `NestedFileCache`
- Display label: `Project Rule (subdir/.rara/rules/file.md)`
- Also reserves `LOCAL_INSTRUCTION_FILE = ".rara/local.md"` for future use

### Upstream reference

Claude Code walks `CLAUDE.md`, `.claude/CLAUDE.md`, `.claude/rules/*.md`
from CWD to root. RARA mirrors this with `AGENTS.md` + `.rara/rules/*.md`.
