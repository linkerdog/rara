# MCP Tool Search

## Problem

大 MCP server（100+ tools）把所有 tool schema 注入每轮 prompt，吃 token、污染 context。

## Design

RARA keeps a volatile per-process cache of `tools/list` results. The cache is
cleared on startup, refreshed from configured MCP servers, and searched on
demand by the stable `mcp_tool_search` tool.

The tool itself is registered in the normal `ToolManager` even when the cache is
empty. This keeps the tool-schema prefix stable across turns; an empty cache
returns a structured empty result instead of changing the visible tool list.

```
session start
  → clear mcp tool cache
  → MCP servers connect
    → list_tools() per server
    → insert tool { name, server, display_name, description, input_schema } into volatile cache
  → agent prompt: stable mcp_tool_search tool visible
  → model calls mcp_tool_search("bash")
    → like-filter on name + description
    → return matching tool schemas
session end
  → process exits; cache is discarded
```

### Cache record

```rust
struct McpToolRecord {
    name: String,          // tool name (searchable)
    server: String,        // which MCP server
    display_name: String,  // "server: name" for display
    input_schema: String,  // JSON schema string
    description: String,   // tool description (searchable)
}
```

### mcp_tool_search tool

```json
{
  "name": "mcp_tool_search",
  "description": "Search MCP tools by keyword. Returns matching tool names, descriptions, and input schemas.",
  "parameters": {
    "query": { "type": "string", "description": "Search query (tool name or capability)" }
  }
}
```

Search query: `like('%{query}%', name) OR like('%{query}%', description)` — substring match, no BM25 or vector.

### Prompt injection

`mcp_tool_search` is a stable tool entrypoint. Full MCP tool schemas should not
be injected into every prompt; the search tool returns matching schemas only
when needed. Keeping the entrypoint stable avoids cache-prefix churn when MCP
servers connect, disconnect, or refresh.

## Implementation plan

1. `src/mcp_tool_cache.rs` — volatile cache insert/search/clear.
2. `src/tools/mcp_tool_search.rs` — registered tool definition over cache.search().
3. Runtime bootstrap — construct one shared cache and register the search tool.
4. `/mcp` refresh path — repopulate the cache from configured stdio servers.
5. Future lifecycle hooks — refresh on connection-manager events for dynamic
   server changes.

## Implementation checkpoint

2026-05-09:

- `McpToolCache` is cloneable and shared between runtime bootstrap, TUI refresh,
  and the tool handler.
- `McpToolRecord` carries `server` provenance; cache insertion normalizes
  `display_name` as `server: tool`.
- Search results are sorted by `(server, name)` before truncation so the same
  cache contents produce deterministic output.
- `mcp_tool_search` implements `Tool`, returns structured JSON, rejects empty
  queries, and is registered by `create_full_tool_manager`.
- Runtime initialization has a regression test that verifies the tool is visible
  in the normal schema list.
