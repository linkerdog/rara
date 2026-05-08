//! MCP tool cache — stores tools from connected MCP servers so the model
//! can discover them via `mcp_tool_search` instead of loading all tool
//! schemas into every prompt.
//!
//! Lifecycle: clear on startup, populated on MCP connect, searched on demand,
//! cleared on shutdown.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A single MCP tool available for search.
#[derive(Debug, Clone)]
pub struct McpToolRecord {
    pub name: String,
    pub server: String,
    pub display_name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// In-memory cache of MCP tool records, keyed by server name.
/// Wrapped in Arc<Mutex<...>> for shared access across tool handlers.
pub struct McpToolCache {
    tools: Arc<Mutex<HashMap<String, Vec<McpToolRecord>>>>,
}

impl McpToolCache {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Replace the tool list for a given server (called on MCP connect).
    pub fn insert_server_tools(&self, server: String, tools: Vec<McpToolRecord>) {
        let mut map = self.tools.lock().unwrap();
        map.insert(server, tools);
    }

    /// Search all cached tools by substring match on name and description.
    pub fn search(&self, query: &str) -> Vec<McpToolRecord> {
        let query = query.to_lowercase();
        let map = self.tools.lock().unwrap();
        let mut results = Vec::new();
        for tools in map.values() {
            for tool in tools {
                if tool.name.to_lowercase().contains(&query)
                    || tool.description.to_lowercase().contains(&query)
                {
                    results.push(tool.clone());
                }
            }
        }
        results.truncate(10); // cap at 10 results
        results
    }

    /// Clear all cached tools (called on startup and shutdown).
    pub fn clear(&self) {
        let mut map = self.tools.lock().unwrap();
        map.clear();
    }

    /// Return true if any tools are cached (used to decide prompt injection).
    pub fn is_empty(&self) -> bool {
        let map = self.tools.lock().unwrap();
        map.is_empty()
    }

    /// For testing: shared Arc clone.
    pub fn share(&self) -> Arc<Mutex<HashMap<String, Vec<McpToolRecord>>>> {
        self.tools.clone()
    }
}
