# Thread Store Warning Cleanup

## Summary

The thread-store warning cleanup keeps the documented thread inspection and
memory-distillation contracts while making the active CLI inspection path use
more of the materialized thread data:

- `rara thread <THREAD_ID>` now prints materialization provenance, lineage,
  rollout event counts, rollout interaction statuses, spawn-agent summaries,
  and compaction replacement metadata.
- `ThreadStore::latest_thread_id` was removed because callers already use
  `latest_thread_summary` and no spec depends on the thin id-only alias.
- `ThreadStore::export_thread_markdown`, `ThreadStore::distill_thread_summary`,
  `ThreadRecorder::flush`, and the summary-record formatter remain as reserved
  contract boundaries with item-level dead-code rationale.
- `src/thread_store.rs` was split into focused `types`, `format`, and
  `recorder` submodules so the main materialization file stays under the
  project source-file size limit.

## Background

`ThreadStore` is the canonical materialization boundary for thread metadata,
transcript history, non-turn rollout events, compaction records, and legacy
fallbacks. Several inspection fields were populated but not displayed by the
CLI, so the compiler correctly reported them as unused in the binary even
though they are part of the thread contract.

The summary-style distillation path is intentionally retained as a compatibility
path. The active product path remains `ThreadStore::distill_thread_memories`,
which extracts multiple durable memory records from loaded thread markdown.

## Validation

```bash
cargo fmt
cargo check --locked --workspace --all-targets
cargo test --locked thread_cli::tests::
cargo test --locked thread_store::tests::
```

## Follow-Ups

- Continue warning cleanup in `AgentEvent` and the TUI command/event palettes.
