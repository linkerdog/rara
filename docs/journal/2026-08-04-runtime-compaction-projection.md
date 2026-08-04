# Runtime-Owned Compaction Projection

## What changed

Compaction now emits a structured runtime session event after the existing
durable compaction record is written. The event carries the compaction count,
before/after token estimates, summary, and recent files. The TUI consumes it as
a typed transcript entry and updates its runtime snapshot.

## Why

OpenCode treats compaction as a first-class session message rather than only a
status counter. RARA already had durable compaction records, so the runtime
event is a projection of that source of truth instead of a second persistence
mechanism.

## Trade-offs

The legacy role/message transcript fields remain unchanged for restore and
export compatibility. The typed payload is currently additive; renderer
cleanup that removes remaining role matching is a separate step.

## Verification

- `cargo fmt --all`
- `cargo test --bin rara tui::runtime::events --no-fail-fast`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`

## Remaining work

- Render compaction entries with a dedicated OpenCode-style timeline cell.
- Correlate tool use, progress, and result events with one runtime-owned tool
  identity.
- Move completed-tool summaries fully onto typed session projection data.
