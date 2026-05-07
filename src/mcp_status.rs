use std::collections::BTreeMap;
use std::path::PathBuf;

use rara_persistence::redaction::{redact_secrets, sanitize_url_for_display};
use serde::{Deserialize, Serialize};

use crate::config::{
    McpRegistry, McpServerConfig, McpServerScope, McpServerTransport, SourcedMcpServerConfig,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum McpConnectionState {
    Configured,
    Connecting,
    Connected,
    Disconnected,
    Refreshing,
    Reconnecting,
    Failed,
    Disabled,
}

impl McpConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Refreshing => "refreshing",
            Self::Reconnecting => "reconnecting",
            Self::Failed => "failed",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    Stdio,
    StreamableHttp,
}

impl McpTransportKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::StreamableHttp => "streamable-http",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub scope: McpServerScope,
    pub source_path: PathBuf,
    pub state: McpConnectionState,
    pub transport: McpTransportKind,
    pub display_target: String,
    pub required: bool,
    pub enabled: bool,
    pub allowed_tools_count: Option<usize>,
    pub disabled_tools_count: Option<usize>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStatusSnapshot {
    pub servers: Vec<McpServerStatus>,
}

impl McpStatusSnapshot {
    pub fn from_registry(registry: &McpRegistry) -> Self {
        let servers = registry
            .servers
            .iter()
            .map(|(name, sourced)| server_status(name, sourced))
            .collect();

        Self { servers }
    }
}

pub fn format_mcp_status(snapshot: &McpStatusSnapshot) -> String {
    let mut lines = vec!["MCP Servers".to_string()];

    if snapshot.servers.is_empty() {
        lines.push(String::new());
        lines.push("No MCP servers configured.".to_string());
        return lines.join("\n");
    }

    let mut groups: BTreeMap<(McpServerScope, PathBuf), Vec<&McpServerStatus>> = BTreeMap::new();
    for server in &snapshot.servers {
        groups
            .entry((server.scope, server.source_path.clone()))
            .or_default()
            .push(server);
    }

    for ((scope, path), mut servers) in groups {
        servers.sort_by(|left, right| left.name.cmp(&right.name));
        lines.push(String::new());
        lines.push(format!(
            "{} ({})",
            scope_heading(scope),
            path.to_string_lossy()
        ));
        for server in servers {
            lines.push(format!(
                "- {} [{}] {}: {}{}",
                server.name,
                server.state.label(),
                server.transport.label(),
                server.display_target,
                status_flags(server)
            ));
            let tool_filters = tool_filter_parts(server);
            if !tool_filters.is_empty() {
                lines.push(format!("  tools: {}", tool_filters.join(", ")));
            }
            if let Some(error) = &server.last_error {
                lines.push(format!("  error: {error}"));
            }
        }
    }

    lines.join("\n")
}

fn server_status(name: &str, sourced: &SourcedMcpServerConfig) -> McpServerStatus {
    let (transport, display_target) = transport_display(&sourced.config);
    McpServerStatus {
        name: name.to_string(),
        scope: sourced.source.scope,
        source_path: sourced.source.path.clone(),
        state: if sourced.config.enabled {
            McpConnectionState::Configured
        } else {
            McpConnectionState::Disabled
        },
        transport,
        display_target,
        required: sourced.config.required,
        enabled: sourced.config.enabled,
        allowed_tools_count: sourced.config.enabled_tools.as_ref().map(Vec::len),
        disabled_tools_count: sourced.config.disabled_tools.as_ref().map(Vec::len),
        last_error: None,
    }
}

fn transport_display(config: &McpServerConfig) -> (McpTransportKind, String) {
    match &config.transport {
        McpServerTransport::Stdio { command, args, .. } => {
            let mut parts = vec![quote_stdio_display_part(command)];
            parts.extend(args.iter().map(|arg| quote_stdio_display_part(arg)));
            (McpTransportKind::Stdio, redact_secrets(parts.join(" ")))
        }
        McpServerTransport::StreamableHttp { url, .. } => (
            McpTransportKind::StreamableHttp,
            sanitize_url_for_display(url),
        ),
    }
}

fn scope_heading(scope: McpServerScope) -> &'static str {
    match scope {
        McpServerScope::Project => "Project",
        McpServerScope::Local => "Local",
        McpServerScope::User => "User",
        McpServerScope::Enterprise => "Enterprise",
        McpServerScope::Builtin => "Builtin",
    }
}

fn status_flags(server: &McpServerStatus) -> String {
    if server.required {
        " (required)".to_string()
    } else {
        String::new()
    }
}

fn tool_filter_parts(server: &McpServerStatus) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(count) = server.allowed_tools_count {
        parts.push(format!("allow {count}"));
    }
    if let Some(count) = server.disabled_tools_count {
        parts.push(format!("deny {count}"));
    }
    parts
}

fn quote_stdio_display_part(value: &str) -> String {
    if value.is_empty()
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '\'' | '"' | '\\'))
    {
        format!("'{}'", value.replace('\'', r"'\''"))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{McpConnectionState, McpStatusSnapshot, format_mcp_status};
    use crate::config::{ConfigManager, McpServerScope};

    #[test]
    fn derives_status_snapshot_from_user_and_project_registry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let user_config = dir.path().join("config.toml");
        fs::write(
            &user_config,
            r#"
[mcp_servers.docs]
command = "docs-server"
args = ["--stdio"]
enabled_tools = ["search", "fetch"]
disabled_tools = ["delete"]

[mcp_servers.secret]
command = "secret-server"
args = ["--token=supersecretvalue"]

[mcp_servers.remote]
url = "https://example.com/mcp?token=secret"
enabled = false
"#,
        )
        .expect("write user config");

        let project = dir.path().join("project");
        fs::create_dir_all(&project).expect("project dir");
        fs::write(
            project.join(".mcp.json"),
            r#"{
  "mcpServers": {
    "repo": {
      "command": "repo-mcp",
      "args": ["--root", ".", "--label", "my repo"],
      "required": true
    }
  }
}"#,
        )
        .expect("write project config");

        let manager = ConfigManager {
            path: dir.path().join("config.json"),
        };
        let registry = manager
            .load_mcp_registry_for_project(&project)
            .expect("registry");
        let snapshot = McpStatusSnapshot::from_registry(&registry);

        assert_eq!(snapshot.servers.len(), 4);
        let repo = snapshot
            .servers
            .iter()
            .find(|server| server.name == "repo")
            .expect("repo server");
        assert_eq!(repo.scope, McpServerScope::Project);
        assert_eq!(repo.state, McpConnectionState::Configured);
        assert!(repo.required);
        assert_eq!(repo.display_target, "repo-mcp --root . --label 'my repo'");

        let remote = snapshot
            .servers
            .iter()
            .find(|server| server.name == "remote")
            .expect("remote server");
        assert_eq!(remote.state, McpConnectionState::Disabled);
        assert_eq!(
            remote.display_target,
            "https://example.com/mcp?token=%3Credacted%3E"
        );

        let secret = snapshot
            .servers
            .iter()
            .find(|server| server.name == "secret")
            .expect("secret server");
        assert_eq!(
            secret.display_target,
            "secret-server --token=[REDACTED_SECRET]"
        );
        assert!(!secret.display_target.contains("supersecretvalue"));

        assert_eq!(manager.config_toml_path(), user_config);
    }

    #[test]
    fn formats_status_grouped_by_scope_and_source_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let user_config = dir.path().join("config.toml");
        fs::write(
            &user_config,
            r#"
[mcp_servers.docs]
command = "docs-server"
args = ["--stdio"]
enabled_tools = ["search"]

[mcp_servers.deny]
command = "deny-server"
disabled_tools = ["delete"]

[mcp_servers.remote]
url = "https://example.com/mcp?token=secret"
enabled = false
"#,
        )
        .expect("write user config");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).expect("project dir");
        fs::write(
            project.join(".mcp.json"),
            r#"{
  "mcpServers": {
    "repo": {
      "command": "repo-mcp",
      "required": true
    }
  }
}"#,
        )
        .expect("write project config");

        let manager = ConfigManager {
            path: dir.path().join("config.json"),
        };
        let registry = manager
            .load_mcp_registry_for_project(&project)
            .expect("registry");
        let rendered = format_mcp_status(&McpStatusSnapshot::from_registry(&registry));

        assert!(rendered.contains("MCP Servers"));
        assert!(rendered.contains("Project ("));
        assert!(rendered.contains("User ("));
        assert!(rendered.contains("- repo [configured] stdio: repo-mcp (required)"));
        assert!(rendered.contains("- docs [configured] stdio: docs-server --stdio"));
        assert!(rendered.contains("tools: allow 1"));
        assert!(rendered.contains("tools: deny 1"));
        assert!(!rendered.contains("allow 0"));
        assert!(rendered.contains(
            "- remote [disabled] streamable-http: https://example.com/mcp?token=%3Credacted%3E"
        ));
        assert!(!rendered.contains("(disabled)"));
    }

    #[test]
    fn formats_empty_status() {
        let rendered = format_mcp_status(&McpStatusSnapshot { servers: vec![] });

        assert_eq!(rendered, "MCP Servers\n\nNo MCP servers configured.");
    }
}
