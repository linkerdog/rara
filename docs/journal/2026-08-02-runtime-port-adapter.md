# In-Process Runtime Port Adapter

## What changed

Added an in-process `RuntimeClientPort` adapter backed by the session-scoped
`RuntimeEventBus` and a snapshot store. `TuiController` now consumes typed
`RuntimeProjectionEvent` values from that port stream. The task join handle
remains the completion signal, while the old task event receiver is drained
without replaying events after the port has delivered them.

## Why

The previous port and harness slices defined the contract but production TUI
execution still read runtime events from a task-local mpsc channel. That left
the test contract and the real event path divergent and made duplicate event
delivery likely during migration.

Structured events that previously bypassed the bus in the input-control task
are now published directly to the control stream, including status, memory,
approval, and lifecycle-adjacent events. Assistant and tool events retain the
existing raw-event forwarding behavior so external raw subscribers do not
change.

## Trade-offs

The adapter does not execute interactive commands yet. Query, approval,
compact, rebuild, and model changes still use the compatibility task bridge;
the adapter returns an explicit error for command submission rather than
silently treating a non-executing command as successful. Runtime ownership
and command routing will move in a later slice.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
- `cargo test --bin rara tui::runtime_port --no-fail-fast`
- `cargo test --bin rara tui::controller --no-fail-fast`
