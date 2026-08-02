# Runtime Event Mux And Change-Driven Redraw

## What Changed

The TUI event loop now awaits runtime receiver messages and task completion in
one `tokio::select!` activity. Runtime task completion results are passed into
the existing completion reducer without polling or consuming the join handle
twice. A closed runtime receiver waits for the task future before completion
processing, which avoids a busy loop during task shutdown.

Periodic TUI work remains on the 166 ms timer, but the timer only requests a
redraw when autoscroll or shared-task polling changes visible state. Repository
context completion also reports whether its projection changed before marking
the screen dirty.

## Why

Runtime output should wake the TUI immediately, while idle sessions should not
repaint at the timer frequency. Keeping completion in the same mux preserves a
single wakeup path for runtime output, task shutdown, and terminal input.

## Trade-offs

The existing completion reducer remains the compatibility bridge for now. It
accepts an optional completion result so existing focused tests can continue to
exercise the legacy readiness path while the event loop uses the select-owned
result path.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- Focused TUI runtime event and maintainer tests
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`

## Remaining Work

Runtime commands and snapshots still need to move behind `RuntimeClient`; this
change only removes event-receiver and redraw polling from the TUI loop.
