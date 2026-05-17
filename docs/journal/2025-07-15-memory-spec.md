# Memory Spec — Session/Global Files with Summary-Driven Retrieval

## What

Wrote `docs/features/session-global-memory.md` defining a new memory
architecture:

- Session-scoped `.md` files under `~/.rara/memory/sessions/`
- Global durable memory at `~/.rara/memory/global.md`
- `summary.md` as a retrieval router (index of what lives where)
- Tools: `read_memory_file`, `write_memory`, `search_memory`, `promote_to_global`

## Why

RARA currently has no durable per-session memory. Codex and Claude both use
flat-file memory injection. We adopt the same pattern, enhanced with a summary
that guides retrieval — avoiding the need to load all session files into
context every turn.

## Trade-offs

- LanceDB stays for semantic search; files handle structured recall
- Summary.md adds ~1KB to context every turn (acceptable cost)
- Session file format is opinionated Markdown (easy to read/edit manually)

## Remains

- Implementation: 4 phases (~330 lines), then integration into context assembler
- Subagent context optimization (separate spec)
- TUI memory inspection command (`/memory`)
