# Memory Action Notices

## What changed

- Added transient TUI notices for automatic workspace-memory retrieval before a
  model turn.
- Added a transient TUI notice after a session checkpoint is written to memory.
- Mapped memory-control events to transcript notices for record writes, record
  updates, record deletes, label listing, metadata queries, record queries, and
  selection refreshes.
- Kept memory notices content-free: they expose operation names and counts, not
  stored memory content or query text.

## Why

Memory already affects prompt assembly and session continuity, but the TUI did
not show when the runtime queried or wrote memory. A short action notice gives
the operator the same kind of traceability as tool activity without treating
memory operations as tool calls.

## Trade-offs

The notice text is intentionally small and transient. It is useful for local
observability, but it is not a durable audit log and should not become the
source of truth for memory state. Detailed memory state remains available
through the memory store and control-plane events.

## Remaining work

- Consider grouping repeated memory notices if retrieval and write activity
  becomes too noisy during multi-turn tool loops.
