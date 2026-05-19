# App Server Contract Crate Split

## Summary

The app-server/control-plane request contract now lives in a dedicated
`rara-app-server` workspace crate. The main binary crate depends on that
contract crate and keeps only the runtime-facing event adapter logic in
`src/runtime_control.rs`.

## Scope

- Added `crates/rara-app-server` as an independent workspace crate.
- Moved protocol-facing request and provenance types into
  `rara_app_server::runtime_control`.
- Kept `RuntimeEvent`, `RuntimeControlEvent`, and `AgentEvent` conversion logic
  in the main crate because those event payloads still depend on runtime-owned
  MCP, context, todo, memory-promotion, and tool-output types.
- Kept shell approval conversion at the main crate boundary so the protocol
  crate does not depend on `Agent` internals.

## Trade-offs

This is an intentional first slice rather than a full transport/server split.
Moving the dispatcher wholesale would currently pull `Agent`, MCP connection
management, memory handlers, hook registries, and TUI/runtime state back into
the new crate. Extracting the stable request contract first gives external
adapters a small dependency boundary without creating a circular runtime crate.

The app-server crate defines its own control-plane `HookLifecycle` enum instead
of re-exporting the prompt crate type. This keeps the protocol crate dependency
surface limited to `serde` and `serde_json`.

## Validation

- `cargo fmt`
- `cargo test -p rara-app-server`
- `cargo check -p rara`
- `cargo test -p rara runtime_control`

`cargo check -p rara` still reports existing warnings unrelated to this split.

## Follow-up

- Move stable event payloads into `rara-app-server` once the context, MCP, todo,
  and memory-promotion views have protocol-owned summary types.
- Consider moving the dispatcher after the runtime dependencies have narrower
  handle traits.
