//! MCP tool search tool — lets the model discover MCP tools on demand
//! instead of loading all tool schemas into every prompt.

use crate::mcp_tool_cache::McpToolCache;

pub struct McpToolSearch {
    cache: McpToolCache,
}

impl McpToolSearch {
    pub fn new(cache: McpToolCache) -> Self {
        Self { cache }
    }

    /// Search cached MCP tools by keyword. Returns matching tool names,
    /// descriptions, and input schemas.
    pub fn search(&self, query: &str) -> String {
        let results = self.cache.search(query);
        if results.is_empty() {
            return format!("No MCP tools found matching '{query}'");
        }
        let mut output = String::new();
        for tool in &results {
            let schema = serde_json::to_string(&tool.input_schema).unwrap_or_default();
            output.push_str(&format!(
                "- {} — {}\n  Input schema: {}\n",
                tool.display_name, tool.description, schema
            ));
        }
        output
    }
}
