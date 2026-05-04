# MCP Config Registry Checkpoint

## Summary

Added the first MCP runtime slice: a source-aware configuration registry that
loads user `config.toml` and project `.mcp.json` definitions without starting
servers.

## Implemented

- Added `McpRegistry` in `rara-config`.
- Added user-scope TOML loading from `[mcp_servers.*]`.
- Added project-scope JSON loading from `.mcp.json` / `mcpServers`.
- Added explicit source scope and source path on every server.
- Added hard duplicate-name failure across sources.
- Added focused tests for mixed sources, duplicate conflicts, and missing files.

## Design Notes

- RARA borrows Codex's TOML shape for user configuration and Claude Code's
  project `.mcp.json` shape.
- RARA intentionally rejects same-name conflicts instead of applying silent
  precedence. Scope remains useful for display, diagnostics, and future
  permissions.
- Runtime startup, refresh, reconnect, resources, and Tool Search remain behind
  the registry boundary for later phases.

## Validation

- `cargo test -p rara-config mcp -- --nocapture`
- `cargo check`
