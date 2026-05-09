//! MCP tool search tool — lets the model discover MCP tools on demand
//! instead of loading all tool schemas into every prompt.

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError};
use serde_json::{Value, json};

use crate::mcp_tool_cache::McpToolCache;

pub struct McpToolSearch {
    cache: McpToolCache,
}

impl McpToolSearch {
    pub fn new(cache: McpToolCache) -> Self {
        Self { cache }
    }

    fn search_results(&self, query: &str) -> Value {
        let results = self.cache.search(query);
        let tools = results
            .into_iter()
            .map(|tool| {
                json!({
                    "server": tool.server,
                    "name": tool.name,
                    "display_name": tool.display_name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                })
            })
            .collect::<Vec<_>>();
        let is_empty = tools.is_empty();
        json!({
            "query": query,
            "tools": tools,
            "note": if is_empty {
                Some("No cached MCP tools matched the query.")
            } else {
                None
            },
        })
    }
}

#[tool_spec(
    name = "mcp_tool_search",
    description = "Search cached MCP tools by name or capability. Use this before asking for an MCP capability that may not be loaded directly in the prompt.",
    input_schema = {
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Tool name, server name, or capability keyword to search for."
            }
        },
        "required": ["query"],
        "additionalProperties": false
    }
)]
#[async_trait]
impl Tool for McpToolSearch {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let query = input["query"]
            .as_str()
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .ok_or_else(|| ToolError::InvalidInput("query".into()))?;
        Ok(self.search_results(query))
    }
}

#[cfg(test)]
mod tests {
    use rara_mcp_client::McpToolRecord;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn search_returns_stable_structured_tool_records() {
        let cache = McpToolCache::new();
        cache.insert_server_tools(
            "zeta".to_string(),
            vec![McpToolRecord {
                server: String::new(),
                name: "read_file".to_string(),
                display_name: "read_file".to_string(),
                description: "Read workspace files".to_string(),
                input_schema: json!({"type": "object"}),
            }],
        );
        cache.insert_server_tools(
            "alpha".to_string(),
            vec![McpToolRecord {
                server: String::new(),
                name: "read_doc".to_string(),
                display_name: "read_doc".to_string(),
                description: "Read docs".to_string(),
                input_schema: json!({"type": "object"}),
            }],
        );

        let tool = McpToolSearch::new(cache);
        let result = tool
            .call(json!({ "query": "read" }))
            .await
            .expect("search should succeed");
        let tools = result["tools"].as_array().expect("tools array");

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["display_name"], "alpha: read_doc");
        assert_eq!(tools[1]["display_name"], "zeta: read_file");
    }

    #[tokio::test]
    async fn search_rejects_empty_query() {
        let tool = McpToolSearch::new(McpToolCache::new());
        let error = tool
            .call(json!({ "query": " " }))
            .await
            .expect_err("empty query should fail");

        assert!(error.to_string().contains("query"));
    }
}
