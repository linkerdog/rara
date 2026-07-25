# Plugin MCP Registry

## Summary

Plugin `.mcp.json` files now feed RARA's shared MCP registry instead of staying
as parsed plugin metadata. Runtime bootstrap combines user, project, and plugin
MCP definitions before constructing the MCP connection manager, so every
surface that consumes the registry sees the same plugin-provided servers.

## Key Decisions

- Plugin MCP entries use the existing `McpRegistry` boundary and a dedicated
  `plugin` source scope.
- The source path for each plugin MCP server is the plugin `.mcp.json` file.
- Plugin stdio MCP servers receive `cwd` set to the plugin root so relative
  plugin launcher commands resolve from the installed plugin directory.
- Plugin stdio MCP servers with explicit relative `cwd` values resolve those
  paths from the plugin root.
- Duplicate MCP server names across user, project, and plugin sources remain a
  hard registry error rather than a precedence override.
- Skill invocation/reload and plugin agent definitions are completed by the
  later extension completion slice.

## Validation

- `cargo test plugin_middleware::tests::appends_plugin_mcp_configs_with_plugin_source_metadata -- --nocapture`
- `cargo test plugin_middleware::tests::plugin_mcp_configs_resolve_relative_cwd_from_plugin_root -- --nocapture`
- `cargo test plugin_middleware::tests::plugin_mcp_configs_skip_mcp_json_directories -- --nocapture`
- `cargo test plugin_middleware::tests::plugin_mcp_configs_fail_on_duplicate_server_names -- --nocapture`

## Follow-Ups

- Plugin skill invocation/reload, plugin agents, and structured readiness were
  completed in `docs/journal/2026-07-25-extension-completion.md`.
