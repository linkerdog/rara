# TUI State Method Split

## Summary

The `TuiApp` state implementation is now split across focused child modules
instead of keeping runtime snapshot, overlay, and pending interaction state
logic inside `src/tui/state/mod.rs`.

## Scope

- Moved runtime snapshot synchronization into `state/runtime_snapshot.rs`.
- Moved overlay open/dismiss state transitions into `state/overlay_state.rs`.
- Moved pending interaction and approval state helpers into
  `state/pending_interaction.rs`.
- Reduced `src/tui/state/mod.rs` from 2087 lines to 1442 lines.

## Validation

```bash
cargo fmt
cargo test tui::state::tests -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
cargo test --locked
git diff --check
```

## Follow-Ups

- `src/tui/state/mod.rs` still owns provider/model selection and app
  construction. Those can be split in later file-size work if needed.
