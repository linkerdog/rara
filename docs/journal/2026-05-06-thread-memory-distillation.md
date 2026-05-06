# 2026-05-06 · Thread Memory Distillation

## Context

Thread distillation previously produced a single summary-style
`ThreadDistill` memory record. That was useful as a compatibility checkpoint,
but it did not satisfy the memory product contract: memories should be
independently useful durable units such as decisions, facts, procedures,
insights, and experiences.

## Implementation

- Added `MemoryDistiller` as the LLM-assisted extraction boundary for loaded
  thread markdown.
- Added `ThreadStore::distill_thread_memories`, which loads a thread, asks the
  distiller for 2-8 memory drafts, deduplicates against existing memory search
  hits and same-batch duplicates, and persists the resulting `MemoryRecord`s
  through `MemoryStore`.
- Kept `ThreadStore::distill_thread_summary` as the compatibility path for a
  single summary-style memory record.
- Preserved thread provenance on generated records through `session_id`,
  `thread_id`, and a whole-thread source span.
- Updated the distillation prompt and default agent memory guidance so old
  memories are treated as historical context rather than current truth. If a
  thread proves that current behavior is stale, incomplete, or poorly designed,
  RARA should capture the corrected durable fact or the need for a small
  purpose-built tool rather than preserving the old state.

## Validation

- `cargo test memory_distiller -- --nocapture`
- `cargo test distill_thread -- --nocapture`

## Follow-up

- Add long-thread chunking for threads with many messages.
- Add finer source-span attribution per extracted memory.
- Wire periodic session-shard promotion into this distillation path.
