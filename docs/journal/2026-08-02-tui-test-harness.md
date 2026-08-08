# TUI Test Harness

## What changed

Added a test-only `FakeRuntimeClient` and `TuiHarness` under
`src/tui/testing`. The fake implements `RuntimeClientPort`, stores a scripted
snapshot, publishes typed projection events, records commands, and exposes
disconnect/reconnect controls. The harness applies events through the
production TUI reducer and renders through the production renderer backed by
an in-memory terminal adapter.

The first regression covers snapshot synchronization, structured session and
assistant events, typed cancellation, completion, disconnect, reconnect, and
rendering without wall-clock sleeps or runtime construction.

## Why

The runtime port introduced in the previous migration slice needs a shared
fixture before TUI concurrency and lifecycle behavior can be tested without
assembling an `Agent`, registries, channels, and real task handles in every
case. Keeping the fake behind the port also makes accidental dependencies on
runtime ownership visible at compile time.

## Trade-offs

The project uses a custom terminal wrapper whose backend requires `io::Error`
and `Write`, while Ratatui's `TestBackend` uses `Infallible` and does not
implement `Write`. The harness therefore uses a small test-only adapter that
delegates to `TestBackend` and preserves its in-memory buffer semantics.

The production controller is not yet constructed from `RuntimeClientPort`;
this slice deliberately establishes the reusable fake and projection/render
fixture first. Virtual time and command routing are separate follow-ups.

## Verification

- `cargo fmt --all`
- `cargo test --bin rara tui::testing --no-fail-fast`
