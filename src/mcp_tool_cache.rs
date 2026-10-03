//! MCP tool cache — stores tools from connected MCP servers so the model
//! can discover them via `mcp_tool_search` instead of loading all tool
//! schemas into every prompt.
//!
//! Lifecycle: clear on startup, populated on MCP connect, searched on demand,
//! cleared on shutdown.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use rara_mcp_client::{HttpProxyPolicy, McpToolRecord};

use crate::config::McpServerTransport;

/// Header sources declared by a streamable-HTTP MCP server config.
struct HttpHeaderSources<'a> {
    bearer_token_env_var: Option<&'a str>,
    headers: Option<&'a BTreeMap<String, String>>,
    env_headers: Option<&'a BTreeMap<String, String>>,
}

/// Request headers for a streamable-HTTP MCP server, plus unread env vars.
struct ResolvedHttpHeaders {
    headers: Vec<(String, String)>,
    missing_env_vars: Vec<String>,
}

/// Resolve request headers for a streamable-HTTP MCP server.
///
/// Header-name priority from lowest to highest is `bearer_token_env_var`,
/// `env_http_headers`, then `http_headers`, so an explicit static header always
/// wins over a value read from the environment. `bearer_token_env_var` is sent
/// as `Authorization: Bearer <token>`, matching the transport contract.
fn resolve_http_headers(
    sources: HttpHeaderSources<'_>,
    lookup_env: impl Fn(&str) -> Option<String>,
) -> ResolvedHttpHeaders {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut missing_env_vars: Vec<String> = Vec::new();

    if let Some(env_var) = sources.bearer_token_env_var {
        match lookup_env(env_var) {
            Some(token) => {
                headers.insert("authorization".to_string(), format!("Bearer {token}"));
            }
            None => missing_env_vars.push(env_var.to_string()),
        }
    }
    if let Some(env_headers) = sources.env_headers {
        for (name, env_var) in env_headers {
            match lookup_env(env_var) {
                Some(value) => {
                    headers.insert(name.to_ascii_lowercase(), value);
                }
                None => missing_env_vars.push(env_var.clone()),
            }
        }
    }
    if let Some(static_headers) = sources.headers {
        for (name, value) in static_headers {
            headers.insert(name.to_ascii_lowercase(), value.clone());
        }
    }

    ResolvedHttpHeaders {
        headers: headers.into_iter().collect(),
        missing_env_vars,
    }
}

/// Redact the complete diagnostic chain at the runtime logging boundary.
fn probe_error_for_display(error: &anyhow::Error) -> String {
    rara_persistence::redaction::redact_secrets(format!("{error:#}"))
}

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
            let tools = match &entry.config.transport {
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
                    rara_mcp_client::list_stdio_tools(cmd, argv, env_map, cwd.clone()).await
                }
                McpServerTransport::StreamableHttp {
                    url,
                    bearer_token_env_var,
                    http_headers,
                    env_http_headers,
                    ..
                } => {
                    let resolved = resolve_http_headers(
                        HttpHeaderSources {
                            bearer_token_env_var: bearer_token_env_var.as_deref(),
                            headers: http_headers.as_ref(),
                            env_headers: env_http_headers.as_ref(),
                        },
                        |env_var| std::env::var(env_var).ok(),
                    );
                    if !resolved.missing_env_vars.is_empty() {
                        log::warn!(
                            "[mcp-tool-cache] {name}: unset environment variable(s) {} for MCP request headers",
                            resolved.missing_env_vars.join(", ")
                        );
                    }
                    let proxy_policy = if entry.config.transport.bypasses_proxy() {
                        HttpProxyPolicy::Bypass
                    } else {
                        HttpProxyPolicy::System
                    };
                    rara_mcp_client::list_http_tools(url.clone(), resolved.headers, proxy_policy)
                        .await
                }
            };

            match tools {
                Ok(tools) => {
                    self.insert_server_tools(name.clone(), tools);
                }
                Err(e) => {
                    log::warn!(
                        "[mcp-tool-cache] failed to list tools from {name}: {}",
                        probe_error_for_display(&e)
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_errors_redact_credentials_in_nested_causes() {
        let error = anyhow::anyhow!(
            "request failed for https://user:pass@example.com/mcp?token=short&exaApiKey=private&mode=search#fragment"
        ).context("MCP tools/list failed");
        let rendered = probe_error_for_display(&error);
        assert!(rendered.contains("MCP tools/list failed: request failed"));
        assert!(rendered.contains("example.com/mcp"));
        for secret in ["user", "pass", "short", "private", "fragment"] {
            assert!(!rendered.contains(secret), "leaked credential: {secret}");
        }
    }

    #[test]
    fn env_authorization_overrides_bearer_without_static_header() {
        let env_headers = BTreeMap::from([("AuThOrIzAtIoN".into(), "AUTH".into())]);
        let resolved = resolve_http_headers(
            HttpHeaderSources {
                bearer_token_env_var: Some("BEARER"),
                headers: None,
                env_headers: Some(&env_headers),
            },
            |name| Some(format!("env-{name}")),
        );
        assert_eq!(
            resolved.headers,
            [("authorization".into(), "env-AUTH".into())]
        );
    }

    #[test]
    fn review_regression_mcp_header_precedence_is_case_insensitive() {
        let env_headers = BTreeMap::from([
            ("Authorization".into(), "AUTH".into()),
            ("X-Custom".into(), "CUSTOM".into()),
        ]);
        let static_headers = BTreeMap::from([
            ("AUTHORIZATION".into(), "static-auth".into()),
            ("x-custom".into(), "static-custom".into()),
        ]);
        let resolved = resolve_http_headers(
            HttpHeaderSources {
                bearer_token_env_var: Some("BEARER"),
                headers: Some(&static_headers),
                env_headers: Some(&env_headers),
            },
            |name| Some(format!("env-{name}")),
        );
        assert_eq!(resolved.headers.len(), 2);
        for (name, value) in [
            ("authorization", "static-auth"),
            ("x-custom", "static-custom"),
        ] {
            let actual = resolved
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name));
            assert_eq!(actual.map(|(_, value)| value.as_str()), Some(value));
        }
    }

    #[test]
    fn resolve_http_headers_merges_bearer_env_headers_and_static_headers() {
        let env_headers = BTreeMap::from([
            ("X-NMEM-API-Key".to_string(), "NMEM_API_KEY".to_string()),
            ("X-Nmem-Space-Id".to_string(), "NMEM_SPACE".to_string()),
        ]);
        let static_headers = BTreeMap::from([("APP".to_string(), "RARA".to_string())]);

        let resolved = resolve_http_headers(
            HttpHeaderSources {
                bearer_token_env_var: Some("NMEM_API_KEY"),
                headers: Some(&static_headers),
                env_headers: Some(&env_headers),
            },
            |name| (name == "NMEM_API_KEY").then(|| "nmem_ck_test".to_string()),
        );

        assert_eq!(
            resolved.headers,
            vec![
                ("app".to_string(), "RARA".to_string()),
                (
                    "authorization".to_string(),
                    "Bearer nmem_ck_test".to_string()
                ),
                ("x-nmem-api-key".to_string(), "nmem_ck_test".to_string()),
            ]
        );
        assert_eq!(resolved.missing_env_vars, vec!["NMEM_SPACE".to_string()]);
    }

    #[test]
    fn resolve_http_headers_prefers_static_headers_over_env_values() {
        let static_headers =
            BTreeMap::from([("Authorization".to_string(), "Bearer explicit".to_string())]);

        let resolved = resolve_http_headers(
            HttpHeaderSources {
                bearer_token_env_var: Some("NMEM_API_KEY"),
                headers: Some(&static_headers),
                env_headers: None,
            },
            |_| Some("env-token".to_string()),
        );

        assert_eq!(
            resolved.headers,
            vec![("authorization".to_string(), "Bearer explicit".to_string())]
        );
        assert!(resolved.missing_env_vars.is_empty());
    }

    #[test]
    fn resolve_http_headers_reports_unset_bearer_env_var() {
        let resolved = resolve_http_headers(
            HttpHeaderSources {
                bearer_token_env_var: Some("NMEM_API_KEY"),
                headers: None,
                env_headers: None,
            },
            |_| None,
        );

        assert!(resolved.headers.is_empty());
        assert_eq!(resolved.missing_env_vars, vec!["NMEM_API_KEY".to_string()]);
    }
}
