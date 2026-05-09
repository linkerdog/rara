# MCP Tool Search Registration

## Context

The MCP Tool Search files existed, but the tool was not part of the normal
runtime tool set. That meant cached MCP tools could not be discovered by the
agent even after `/mcp` populated the cache.

## Changes

- Registered `mcp_tool_search` through the standard `ToolManager`.
- Shared one `McpToolCache` between runtime bootstrap, TUI MCP refresh, and the
  tool handler.
- Added server provenance to `McpToolRecord` and normalized display names as
  `server: tool`.
- Sorted search results by server and tool name to keep output deterministic.
- Returned structured JSON from `mcp_tool_search` instead of a prose block.
- Marked the MCP Tool Search TODO as implemented.

## Verification

- `cargo fmt --check`
- `cargo test search_returns_stable_structured_tool_records --locked`
- `cargo test initialize_rara_context_registers_mcp_tool_search --locked`

## Follow-up

Connection-manager driven MCP refresh can replace the current `/mcp`-triggered
cache population once dynamic server lifecycle events are fully wired.
