//! Owned MCP stdio connections and bounded tool catalogue discovery.
//!
//! Used by the MCP Tool Search feature to build the tool index at startup.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;
use tokio::process::Command;

mod bounded_reader;
mod connection;

pub use connection::StdioConnection;
pub use rmcp::model::{CallToolResponse, Tool as McpToolDefinition};

/// A single MCP tool record for caching.
#[derive(Debug, Clone)]
pub struct McpToolRecord {
    pub server: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Connect to an MCP server via stdio and list all available tools.
///
/// # Arguments
/// * `command` — the executable to spawn (e.g., "npx")
/// * `args` — arguments for the command (e.g., ["-y", "@modelcontextprotocol/server-filesystem"])
/// * `env` — environment variables to pass
/// * `cwd` — working directory (optional)
pub async fn list_stdio_tools(
    command: OsString,
    args: Vec<OsString>,
    env: HashMap<String, String>,
    cwd: Option<PathBuf>,
) -> Result<Vec<McpToolRecord>> {
    let mut cmd = Command::new(&command);
    cmd.args(&args);
    for (key, value) in &env {
        cmd.env(key, value);
    }
    if let Some(dir) = &cwd {
        cmd.current_dir(dir);
    }
    let mut connection = StdioConnection::connect(cmd).await?;
    let tools = connection
        .tools()
        .iter()
        .map(|t| McpToolRecord {
            server: String::new(),
            name: t.name.to_string(),
            display_name: t.name.to_string(),
            description: t.description.as_deref().unwrap_or_default().to_owned(),
            input_schema: serde_json::Value::Object((*t.input_schema).clone()),
        })
        .collect();
    connection.shutdown().await?;
    Ok(tools)
}
