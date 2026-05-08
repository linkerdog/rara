# MCP Tool Search

## Problem

大 MCP server（100+ tools）把所有 tool schema 注入每轮 prompt，吃 token、污染 context。

## Design

LanceDB 临时表 — 启动清空，退出清空。运行时 MCP server 连接后缓存 `tools/list` 结果。

```
session start
  → drop mcp_tools table
  → MCP servers connect
    → list_tools() per server
    → insert tool { name, server, description, input_schema } into lance
  → agent prompt: only mcp_tool_search tool visible
  → model calls mcp_tool_search("bash")
    → like-filter on name + description
    → return matching tool schemas
session end
  → drop mcp_tools table
```

### LanceDB schema

```rust
#[derive(LanceTable)]
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

When `mcp_tools` table is non-empty, inject only `mcp_tool_search` instead of full MCP tool list. After each search call, result tools are available for the current turn.

## Implementation plan

1. `src/mcp_tool_cache.rs` — LanceDB table create/insert/search/drop
2. `src/tools/mcp_tool_search.rs` — tool definition, calls cache.search()
3. Hook into MCP server connection lifecycle — refresh cache on connect
4. Hook into session start/end — drop table on start/exit
5. Update prompt builder — inject mcp_tool_search instead of full tool list when cache is populated
