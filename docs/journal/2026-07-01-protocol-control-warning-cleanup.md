# Protocol Control Warning Cleanup

## Summary

The protocol prompt-source registry no longer exposes older static listing and
manual turn-advance helpers. Query-time snapshots now use
`list_prompt_sources_for_query`, which is the single path that converts active
protocol sources into prompt-runtime sources, advances turn-limited lifetimes,
and emits lifecycle events.

The TUI runtime now builds `MemoryControlHandler` with the active agent
`MemoryStore`, so local control-plane memory requests use the durable
store-backed path instead of the scaffold-only event path.

## Key Decisions

- Remove unused prompt-source helper APIs rather than suppressing them, because
  they were superseded by the query-time snapshot API.
- Keep protocol skill precedence hints as a reserved field because the runtime
  control-plane spec still requires advisory ordering for external skill roots.
- Keep the structured event subscription API reserved for ACP, Wire, and
  appserver consumers.

## Validation

```bash
cargo fmt
cargo check --locked --workspace --all-targets
cargo test --locked prompt_source_registry
cargo test --locked protocol_prompt_registry_feeds_prompt_runtime_for_query
```

## Follow-Ups

- Continue warning cleanup in the session promotion, thread materialization,
  and TUI command/event palettes.
