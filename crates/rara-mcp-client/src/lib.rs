//! Minimal MCP client — connects to a configured MCP server, calls
//! `tools/list`, and returns tool definitions for caching.
//!
//! Supports stdio child-process servers and streamable-HTTP servers.
//!
//! Used by the MCP Tool Search feature to build the tool index at startup.
//! Follows the same `rmcp`-based pattern used by Claude Code and Codex.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use http::{HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::Tool;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
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

    Ok(tool_records(tools_result.tools))
}

/// Connect to an MCP server over streamable HTTP and list all available tools.
///
/// # Arguments
/// * `url` — the MCP endpoint URL
/// * `headers` — headers applied to every request, already resolved from static
///   config, env-var-backed headers, and bearer-token sources
pub async fn list_http_tools(
    url: String,
    headers: Vec<(String, String)>,
) -> Result<Vec<McpToolRecord>> {
    let config = StreamableHttpClientTransportConfig::with_uri(url.clone())
        .custom_headers(header_map(&headers)?);
    let transport = StreamableHttpClientTransport::from_config(config);

    let service = timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .context("MCP connect timed out")?
        .with_context(|| format!("Failed to connect to MCP server at {url}"))?;

    let tools_result = service
        .list_tools(Default::default())
        .await
        .context("MCP tools/list failed")?;

    Ok(tool_records(tools_result.tools))
}

fn header_map(headers: &[(String, String)]) -> Result<HashMap<HeaderName, HeaderValue>> {
    headers
        .iter()
        .map(|(name, value)| {
            let parsed_name = HeaderName::from_bytes(name.as_bytes())
                .with_context(|| format!("invalid MCP header name {name:?}"))?;
            let parsed_value = HeaderValue::from_str(value)
                .with_context(|| format!("invalid value for MCP header {name:?}"))?;
            Ok((parsed_name, parsed_value))
        })
        .collect()
}

fn tool_records(tools: Vec<Tool>) -> Vec<McpToolRecord> {
    tools
        .into_iter()
        .map(|t| McpToolRecord {
            server: String::new(),
            name: t.name.to_string(),
            display_name: t.name.to_string(),
            description: t.description.map(|d| d.to_string()).unwrap_or_default(),
            input_schema: serde_json::Value::Object((*t.input_schema).clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_map_parses_plain_headers() {
        let headers = header_map(&[("X-NMEM-API-Key".to_string(), "token".to_string())]).ok();

        assert_eq!(headers.as_ref().map(HashMap::len), Some(1));
        assert_eq!(
            headers
                .as_ref()
                .and_then(|map| map.get(&HeaderName::from_static("x-nmem-api-key"))),
            Some(&HeaderValue::from_static("token"))
        );
    }

    #[test]
    fn header_map_rejects_invalid_header_name() {
        let err = header_map(&[("bad header".to_string(), "value".to_string())]).err();

        assert_eq!(
            err.map(|err| err.to_string()),
            Some("invalid MCP header name \"bad header\"".to_string())
        );
    }

    #[test]
    fn header_map_rejects_invalid_header_value() {
        let err = header_map(&[("x-test".to_string(), "bad\nvalue".to_string())]).err();

        assert_eq!(
            err.map(|err| err.to_string()),
            Some("invalid value for MCP header \"x-test\"".to_string())
        );
    }
}
