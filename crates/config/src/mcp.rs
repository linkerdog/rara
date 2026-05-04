use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerScope {
    Project,
    Local,
    User,
    Enterprise,
    Builtin,
}

impl McpServerScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Local => "local",
            Self::User => "user",
            Self::Enterprise => "enterprise",
            Self::Builtin => "builtin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerSource {
    pub scope: McpServerScope,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRegistry {
    pub servers: BTreeMap<String, SourcedMcpServerConfig>,
}

impl McpRegistry {
    pub fn empty() -> Self {
        Self {
            servers: BTreeMap::new(),
        }
    }

    fn insert_source(
        &mut self,
        source: McpServerSource,
        servers: BTreeMap<String, McpServerConfig>,
    ) -> Result<()> {
        for (name, config) in servers {
            if let Some(existing) = self.servers.get(&name) {
                bail!(
                    "MCP server `{name}` is defined in both {} ({}) and {} ({}); remove one definition",
                    existing.source.scope.label(),
                    existing.source.path.display(),
                    source.scope.label(),
                    source.path.display()
                );
            }
            self.servers.insert(
                name,
                SourcedMcpServerConfig {
                    config,
                    source: source.clone(),
                },
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcedMcpServerConfig {
    pub config: McpServerConfig,
    pub source: McpServerSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct McpServerConfig {
    #[serde(flatten)]
    pub transport: McpServerTransport,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub supports_parallel_tool_calls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_timeout_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum McpServerTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<BTreeMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
    },
    StreamableHttp {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_token_env_var: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_headers: Option<BTreeMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env_http_headers: Option<BTreeMap<String, String>>,
    },
}

const fn default_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct TomlMcpConfig {
    #[serde(default)]
    mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Deserialize)]
struct ProjectMcpConfig {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, McpServerConfig>,
}

pub fn load_mcp_registry(user_config_toml: &Path, project_root: &Path) -> Result<McpRegistry> {
    let mut registry = McpRegistry::empty();
    let project_config = project_root.join(".mcp.json");

    append_servers_from_path(
        &mut registry,
        user_config_toml,
        McpServerScope::User,
        |content| toml::from_str::<TomlMcpConfig>(content).map(|config| config.mcp_servers),
    )?;
    append_servers_from_path(
        &mut registry,
        &project_config,
        McpServerScope::Project,
        |content| {
            serde_json::from_str::<ProjectMcpConfig>(content).map(|config| config.mcp_servers)
        },
    )?;

    Ok(registry)
}

fn append_servers_from_path<E>(
    registry: &mut McpRegistry,
    path: &Path,
    scope: McpServerScope,
    parse: impl FnOnce(&str) -> std::result::Result<BTreeMap<String, McpServerConfig>, E>,
) -> Result<()>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let servers = parse(&content).with_context(|| format!("parse {}", path.display()))?;
    registry.insert_source(
        McpServerSource {
            scope,
            path: path.to_path_buf(),
        },
        servers,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_user_config_toml_and_project_mcp_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let user_config = dir.path().join("config.toml");
        fs::write(
            &user_config,
            r#"
[mcp_servers.docs]
command = "docs-server"
args = ["--stdio"]
enabled_tools = ["search"]

[mcp_servers.remote]
url = "https://example.com/mcp"
bearer_token_env_var = "MCP_TOKEN"
"#,
        )
        .expect("write config.toml");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).expect("project dir");
        fs::write(
            project.join(".mcp.json"),
            r#"{
  "mcpServers": {
    "repo": {
      "command": "repo-mcp",
      "args": ["--root", "."],
      "required": true
    }
  }
}"#,
        )
        .expect("write .mcp.json");

        let registry = load_mcp_registry(&user_config, &project).expect("registry");

        assert_eq!(registry.servers.len(), 3);
        assert_eq!(registry.servers["docs"].source.scope, McpServerScope::User);
        assert_eq!(
            registry.servers["repo"].source.scope,
            McpServerScope::Project
        );
        assert!(registry.servers["repo"].config.required);
        assert_eq!(
            registry.servers["docs"].config.enabled_tools.as_deref(),
            Some(&["search".to_string()][..])
        );
    }

    #[test]
    fn duplicate_server_names_across_sources_fail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let user_config = dir.path().join("config.toml");
        fs::write(
            &user_config,
            r#"
[mcp_servers.docs]
command = "docs-server"
"#,
        )
        .expect("write config.toml");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).expect("project dir");
        fs::write(
            project.join(".mcp.json"),
            r#"{
  "mcpServers": {
    "docs": {
      "command": "other-docs-server"
    }
  }
}"#,
        )
        .expect("write .mcp.json");

        let err = load_mcp_registry(&user_config, &project).expect_err("conflict");

        let message = err.to_string();
        assert!(message.contains("MCP server `docs` is defined in both user"));
        assert!(message.contains("project"));
    }

    #[test]
    fn missing_config_files_produce_empty_registry() {
        let dir = tempfile::tempdir().expect("tempdir");

        let registry =
            load_mcp_registry(&dir.path().join("config.toml"), dir.path()).expect("registry");

        assert!(registry.servers.is_empty());
    }
}
