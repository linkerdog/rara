//! MCP tool cache — stores tools from connected MCP servers so the model
//! can discover them via `mcp_tool_search` instead of loading all tool
//! schemas into every prompt.
//!
//! Lifecycle: clear on startup, populated on MCP connect, searched on demand,
//! cleared on shutdown.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rara_mcp_client::McpToolRecord;

use crate::config::McpRegistry;
use crate::config::McpServerTransport;

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
        results.truncate(10);
        results
    }

    /// Clear all cached tools (called on startup and shutdown).
    pub fn clear(&self) {
        let mut map = self.tools.lock().unwrap();
        map.clear();
    }

    pub fn is_empty(&self) -> bool {
        let map = self.tools.lock().unwrap();
        map.is_empty()
    }

    /// Connect to all configured MCP stdio servers and populate the cache.
    /// Call once at startup; the registry tells us which servers to connect to.
    pub async fn populate_from_registry(&self, registry: &McpRegistry) {
        for (name, entry) in &registry.servers {
            // Only handle stdio transport for now.
            let (command, args, env, cwd) = match &entry.config.transport {
                McpServerTransport::Stdio {
                    command,
                    args,
                    env,
                    cwd,
                } => {
                    let cmd = std::ffi::OsString::from(command);
                    let argv: Vec<std::ffi::OsString> = args.iter().map(|a| a.into()).collect();
                    let env_map: HashMap<String, String> = env
                        .clone()
                        .map(|m| m.into_iter().collect())
                        .unwrap_or_default();
                    (cmd, argv, env_map, cwd.clone())
                }
                _ => continue, // skip HTTP MCP servers for now
            };

            match rara_mcp_client::list_stdio_tools(command, args, env, cwd).await {
                Ok(tools) => {
                    self.insert_server_tools(name.clone(), tools);
                }
                Err(e) => {
                    eprintln!("[mcp-tool-cache] Failed to list tools from {name}: {e}");
                }
            }
        }
    }

    /// For testing: shared Arc clone.
    pub fn share(&self) -> Arc<Mutex<HashMap<String, Vec<McpToolRecord>>>> {
        self.tools.clone()
    }
}
