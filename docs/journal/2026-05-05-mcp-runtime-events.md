# MCP Runtime Events Checkpoint

## Summary

This checkpoint moves MCP status from a TUI-only surface toward the shared
runtime control plane.

## Implemented

- Added `AgentEvent::McpStatusUpdated`.
- Added `RuntimeEvent::Mcp` with `McpEvent::StatusUpdated`.
- Added MCP control request scaffolding for `query_status`, `refresh`, and
  `reconnect`.
- Made `McpStatusSnapshot` and nested status types serializable.
- Published the `/mcp` registry-derived snapshot to `RuntimeEventBus` when
  subscribers exist.
- Published `/mcp` registry load failures as structured MCP events so runtime
  subscribers can clear stale status.
- Kept serialized MCP server targets display-safe by redacting stdio arguments
  and HTTP URL secrets before they enter the snapshot.

## Boundary

This does not start MCP servers or discover MCP tools/resources/prompts. It only
establishes the structured event path that later connection and refresh logic
will reuse.

## References

- Claude-style SDK control path: `mcp_status` request/response.
- Codex-style app-server path: `mcpServerStatus/list` and
  `mcpServer/startupStatus/updated`.

## Validation

- Runtime event serialization test covers the stable wire shape.
- TUI command test verifies `/mcp` publishes a structured status event.
- Runtime-control request serialization tests cover `query_status`, `refresh`,
  and `reconnect`.
- Status derivation tests cover redacted stdio and HTTP display targets.
