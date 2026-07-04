# 2026-07-05 · TUI runtime task completion split

## Summary

The TUI runtime task module was still over the repository's 1000-line file-size
target. This checkpoint splits task completion handling out of
`src/tui/runtime/tasks.rs` while keeping task startup helpers in the original
module.

## Scope

- Added `src/tui/runtime/tasks/completion.rs` for running-task join,
  completion handling, query heartbeat updates, rebuild result handling, OAuth
  completion handling, and model-list completion handling.
- Kept task start functions, lifecycle forwarding helpers, and shared prompt /
  goal helpers in `src/tui/runtime/tasks.rs`.
- Reduced `src/tui/runtime/tasks.rs` from 1309 lines to 747 lines.
- Left `src/tools/pty.rs` open in `docs/todo.md` because its nested test module
  shape still needs a separate extraction plan.

## Validation

Planned validation for this slice:

```bash
cargo fmt
cargo test --locked tui::runtime::tasks::tests
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```

## Follow-Ups

- `src/tools/pty.rs` remains the only active P0 file-size item.
