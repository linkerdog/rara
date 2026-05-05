# Runtime Event Subscribers Checkpoint

## Summary

This checkpoint turns the existing `AgentEvent` bridge into a protocol-ready
subscription surface.

## Implemented

- Added structured runtime-control subscriptions to `RuntimeEventBus`.
- Kept raw `AgentEvent` subscriptions for local compatibility.
- Assigned event ids and monotonic sequence numbers at the shared bus boundary.
- Wrapped events as `RuntimeControlEvent` with provenance before delivery to
  protocol subscribers.
- Routed TUI agent-turn, compact, review, approval-resume, plan-resume, and MCP
  status events through the provenance-aware send path.

## References

- Codex-style app-server thread event subscriptions and typed notifications.
- Claude-style session WebSocket subscription with SDK/control message
  separation.

## Boundary

This does not implement ACP or Wire adapters. It provides the common subscriber
surface those adapters should consume.

## Validation

- Runtime event bus tests cover structured subscription wrapping, sequence
  assignment, raw compatibility, and provenance.
