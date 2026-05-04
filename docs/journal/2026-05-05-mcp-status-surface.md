# MCP Status Surface Checkpoint

## Summary

This checkpoint adds the first runtime-facing MCP status slice on top of the
source-aware registry.

## Implemented

- Added a read-only `McpStatusSnapshot` that derives per-server status from
  `McpRegistry`.
- Added status fields for server name, scope, source path, transport kind,
  display target, enablement, required flag, tool-filter counts, and last error.
- Added `/mcp` as a local TUI command.
- Rendered `/mcp` output grouped by scope and source path.
- Kept the first slice non-spawning: configured servers are reported as
  `configured`, disabled servers as `disabled`, and configuration load failures
  are shown as failures in the transcript.
- Redacted URL query secrets before showing HTTP MCP targets.

## Boundary

This does not start MCP processes, connect HTTP clients, discover tools,
subscribe to resource updates, or auto-reconnect. Those behaviors should update
the same status snapshot shape instead of introducing another model.

## Validation

- Unit tests cover snapshot derivation and grouped `/mcp` text formatting.
- Command tests cover `/mcp` parsing.

## Follow-Up

- Wire the status snapshot into a real connection manager.
- Publish MCP status changes through runtime control events.
- Extend `/mcp` with refresh/reconnect actions after server startup exists.
