//! Minimal MCP stdio client — connects to a configured MCP server process,
//! calls `tools/list`, and returns tool definitions for caching.
//!
//! Used by the MCP Tool Search feature to build the tool index at startup.
//! Follows the same `rmcp`-based pattern used by Claude Code and Codex.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use rmcp::transport::child_process::TokioChildProcess;
use tokio::process::Command;
use tokio::time::timeout;

/// Default timeout for connecting to an MCP server.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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
    cmd.kill_on_drop(true);

    let transport = TokioChildProcess::new(cmd)
        .with_context(|| format!("Failed to create MCP transport for {:?}", command))?;

    let service = timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .context("MCP connect timed out")?
        .with_context(|| format!("Failed to connect to MCP server {:?}", command))?;

    let tools_result = service
        .list_tools(Default::default())
        .await
        .context("MCP tools/list failed")?;

    Ok(tools_result
        .tools
        .into_iter()
        .map(|t| McpToolRecord {
            server: String::new(),
            name: t.name.to_string(),
            display_name: t.name.to_string(),
            description: t.description.map(|d| d.to_string()).unwrap_or_default(),
            input_schema: serde_json::Value::Object((*t.input_schema).clone()),
        })
        .collect())
}
