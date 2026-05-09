# Memory Scope Promotion Rules

## Checkpoint

RARA now has a typed promotion boundary for durable memory records.
`MemoryPromotionTarget` centralizes how thread, session, and workspace
promotions become `NewMemoryRecord`s before they are inserted into
`MemoryStore`.

## Runtime Contract

- Workspace memory may preserve session or thread provenance, but it indexes
  under the workspace scope key.
- Thread memory requires an explicit thread id and indexes under that thread id.
- Session memory requires an explicit session id and indexes under that session
  id.
- Protocol memory writes use the same promotion rules as thread/session
  distillation.

This keeps the scope decision stable and visible before retrieval orchestration
or `/context` explainability consume the record.

## Follow-Up

Periodic session-shard promotion still needs an explicit scheduler and policy
gate. That work should reuse this promotion boundary instead of constructing
memory records ad hoc.
