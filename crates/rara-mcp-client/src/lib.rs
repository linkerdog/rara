//! Owned MCP stdio connections and streamable-HTTP tool catalogue discovery.
//!
//! Supports stdio child-process servers and streamable-HTTP servers.
//!
//! Used by the MCP Tool Search feature to build the tool index at startup.
//! Uses the explicit HTTP client construction pattern from Codex.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use http::{HeaderName, HeaderValue};
use rmcp::model::Tool;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{Peer, RoleClient, ServiceExt};
use tokio::process::Command;
use tokio::time::timeout;

mod bounded_reader;
mod connection;

pub use connection::StdioConnection;
pub use rmcp::model::{CallToolResponse, Tool as McpToolDefinition};

/// Default timeout for connecting to an MCP server.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// One deadline covers every page, including a server that repeats its cursor.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Proxy routing selected by the configuration registry for an HTTP endpoint.
#[derive(Clone, Copy, Debug)]
pub enum HttpProxyPolicy {
    System,
    Bypass,
}

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
    let tools = tool_records(connection.tools());
    connection.shutdown().await?;
    Ok(tools)
}

/// Connect to an MCP server over streamable HTTP and list all available tools.
///
/// # Arguments
/// * `url` — the MCP endpoint URL
/// * `headers` — headers applied to every request, already resolved from static
///   config, env-var-backed headers, and bearer-token sources
/// * `proxy_policy` — the registry's routing decision, including loopback bypass
pub async fn list_http_tools(
    url: String,
    headers: Vec<(String, String)>,
    proxy_policy: HttpProxyPolicy,
) -> Result<Vec<McpToolRecord>> {
    let config =
        StreamableHttpClientTransportConfig::with_uri(url).custom_headers(header_map(&headers)?);
    let builder = reqwest::Client::builder();
    let builder = match proxy_policy {
        HttpProxyPolicy::System => builder,
        HttpProxyPolicy::Bypass => builder.no_proxy(),
    };
    let client = builder.build().context("Failed to build MCP HTTP client")?;
    let transport = StreamableHttpClientTransport::with_client(client, config);

    let service = timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .context("MCP connect timed out")?
        .context("Failed to connect to MCP HTTP server")?;

    list_tools(&service).await
}

async fn list_tools(peer: &Peer<RoleClient>) -> Result<Vec<McpToolRecord>> {
    let tools = timeout(LIST_TIMEOUT, peer.list_all_tools())
        .await
        .context("MCP tools/list timed out")?
        .context("MCP tools/list failed")?;

    Ok(tool_records(&tools))
}

#[cfg(test)]
mod http_tests;

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

fn tool_records(tools: &[Tool]) -> Vec<McpToolRecord> {
    tools
        .iter()
        .map(|t| McpToolRecord {
            server: String::new(),
            name: t.name.to_string(),
            display_name: t.name.to_string(),
            description: t.description.as_deref().unwrap_or_default().to_owned(),
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
