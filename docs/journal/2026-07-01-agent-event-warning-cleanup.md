# Agent Event Warning Cleanup

## Summary

The agent-event warning cleanup wires the reserved runtime-control lifecycle
events into active execution paths:

- Agent model turns now emit `AgentEvent::ModelRequest` before backend calls and
  `AgentEvent::ModelResponse` after provider responses.
- TUI query, review, and compact tasks now publish `AgentEvent::AgentStop` for
  successful or cancelled completion and `AgentEvent::AgentError` for failures.
- `LlmBackend::model_label` provides model names for structured model events
  without changing backend request APIs.
- The unused `RuntimeEventBus::send` convenience wrapper was removed; callers
  now use provenance-aware `send_with_provenance`.

## Background

`AgentEvent` is the raw event bridge used by local TUI compatibility and the
structured runtime-control subscriber surface. The lifecycle and model events
were already mapped into `RuntimeEvent`, hook lifecycle matching, and plugin
middleware, but no runtime path constructed them.

This change keeps those events out of the TUI transcript conversion path while
making them visible to ACP, Wire, appserver, and other structured subscribers.

## Validation

```bash
cargo fmt
cargo check --locked --workspace --all-targets
cargo test --locked emits_model_request_and_response_events
cargo test --locked lifecycle_helper_publishes
cargo test --locked runtime_event_bus::tests::
```

## Follow-Ups

- Continue warning cleanup in the TUI command/event palettes and related state
  display fields.
