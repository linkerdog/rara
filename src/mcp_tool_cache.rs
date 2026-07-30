//! MCP tool cache — stores tools from connected MCP servers so the model
//! can discover them via `mcp_tool_search` instead of loading all tool
//! schemas into every prompt.
//!
//! Lifecycle: clear on startup, populated on MCP connect, searched on demand,
//! cleared on shutdown.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rara_mcp_client::McpToolRecord;

use crate::config::McpServerTransport;

/// In-memory cache of MCP tool records, keyed by server name.
/// Wrapped in Arc<Mutex<...>> for shared access across tool handlers.
#[derive(Clone)]
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
        let tools = tools
            .into_iter()
            .map(|mut tool| {
                tool.server = server.clone();
                tool.display_name = format!("{server}: {}", tool.name);
                tool
            })
            .collect();
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
        results.sort_by(|left, right| {
            left.server
                .cmp(&right.server)
                .then_with(|| left.name.cmp(&right.name))
        });
        results.truncate(10);
        results
    }

    /// Clear all cached tools (called on startup and shutdown).
    pub fn clear(&self) {
        let mut map = self.tools.lock().unwrap();
        map.clear();
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        let map = self.tools.lock().unwrap();
        map.is_empty()
    }

    /// For testing: shared Arc clone.
    pub fn share(&self) -> Arc<Mutex<HashMap<String, Vec<McpToolRecord>>>> {
        self.tools.clone()
    }

    /// Build a cache from an existing shared state (used when spawning).
    pub(crate) fn from_shared(tools: Arc<Mutex<HashMap<String, Vec<McpToolRecord>>>>) -> Self {
        Self { tools }
    }

    /// Same as populate_from_registry but takes owned server data
    /// (Send-safe, can be called from tokio::spawn).
    pub async fn populate_from_registry_owned(
        &self,
        servers: Vec<(
            String,
            std::sync::Arc<crate::config::SourcedMcpServerConfig>,
        )>,
    ) {
        for (name, entry) in servers {
            let (command, args, env, cwd) = match &entry.config.transport {
                McpServerTransport::Stdio {
                    command,
                    args,
                    env,
                    cwd,
                    ..
                } => {
                    let cmd = std::ffi::OsString::from(command);
                    let argv: Vec<std::ffi::OsString> = args.iter().map(|a| a.into()).collect();
                    let env_map: HashMap<String, String> = env
                        .clone()
                        .map(|m| m.into_iter().collect())
                        .unwrap_or_default();
                    (cmd, argv, env_map, cwd.clone())
                }
                _ => continue,
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
}
