# Session Warning Cleanup

## Summary

The session warning cleanup separates active runtime paths from compatibility
and future promotion paths:

- `SessionManager` no longer exposes unused thin aliases for session loading or
  context slicing.
- Legacy history and compaction migration loaders on `SessionManager` are now
  test-only compatibility coverage. Runtime restore and browsing use
  `ThreadStore`.
- Session-shard promotion APIs remain available as reserved manual or
  control-plane entry points, with item-level dead-code rationale linked to the
  memory-records spec.

## Background

`ThreadStore` owns the current thread materialization path, including
transcript-first history restore and legacy compaction backfill. Keeping the
older `SessionManager` read surface compiled into the main binary made those
compatibility helpers look like production APIs even though only tests still
exercise them directly.

## Validation

```bash
cargo fmt
cargo check --locked --workspace --all-targets
cargo test --locked session::tests::
cargo test --locked checkpoints_user_message_before_first_model_turn
cargo test --locked partial_compact_replaces_only_selected_api_round_range
```

## Follow-Ups

- Continue warning cleanup in thread materialization and TUI command/event
  palettes.
