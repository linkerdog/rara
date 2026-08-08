# Runtime Tool Identity Projection

## What changed

The runtime event bus now assigns and correlates tool call IDs for structured
events that do not already provide one. The tracker is keyed by session and
tool name, assigns an ID on `Use`, reuses it for `Progress`, and consumes it on
`Result`.

## Why

OpenCode models tools as stable session parts. TUI and ACP consumers should not
guess which result belongs to which invocation, especially when multiple
sessions or repeated calls use the same tool name.

## Trade-offs

The existing agent event model does not always expose provider-native IDs, so
the runtime uses a deterministic event-derived fallback. Explicit IDs remain
unchanged. The fallback correlation assumes results for a given tool are
observed in invocation order; provider-native IDs should be preferred when
available.

## Verification

- `cargo fmt --all`
- `cargo test --bin rara runtime_event_bus --no-fail-fast`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
