//! Claude Code plugin discovery and loading.
//!
//! Scans plugin directories, reads `.claude-plugin/plugin.json` and
//! `hooks/hooks.json`, produces a loaded Plugin struct ready for
//! registration with the hook runtime.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A loaded Claude Code plugin.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub root: PathBuf,
    pub hooks: Vec<HookHandler>,
    pub mcp_config: Option<McpConfig>,
    pub load_warnings: Vec<String>,
}

/// A single hook handler from hooks.json.
#[derive(Debug, Clone, Deserialize)]
pub struct HookHandler {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub timeout: u64,
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub once: bool,
}

/// Parsed hooks.json content.
#[derive(Debug, Clone, Deserialize)]
struct HooksJson {
    #[serde(default)]
    #[serde(rename = "Stop")]
    stop: Vec<MatcherGroup>,
    #[serde(default)]
    #[serde(rename = "PostToolUse")]
    post_tool_use: Vec<MatcherGroup>,
    #[serde(default)]
    #[serde(rename = "PreToolUse")]
    pre_tool_use: Vec<MatcherGroup>,
    #[serde(default)]
    #[serde(rename = "UserPromptSubmit")]
    user_prompt_submit: Vec<MatcherGroup>,
    #[serde(default)]
    #[serde(rename = "SessionStart")]
    session_start: Vec<MatcherGroup>,
    #[serde(default)]
    #[serde(rename = "SessionEnd")]
    session_end: Vec<MatcherGroup>,
}

#[derive(Debug, Clone, Deserialize)]
struct MatcherGroup {
    #[serde(default)]
    matcher: Option<String>,
    hooks: Vec<HookHandler>,
}

/// Minimal parsed plugin.json (only the fields we need).
#[derive(Debug, Clone, Deserialize)]
struct PluginJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: String,
}

/// Parsed .mcp.json content.
#[derive(Debug, Clone, Deserialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, McpServer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Registered hook handler with its event binding.
#[derive(Debug, Clone)]
pub struct RegisteredHook {
    pub event: HookEvent,
    pub handler: HookHandler,
    pub plugin_name: String,
    pub plugin_root: PathBuf,
}

/// Hook lifecycle events matching hooks.json keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    Stop,
    PostToolUse,
    PreToolUse,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
}

impl HookEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stop => "Stop",
            Self::PostToolUse => "PostToolUse",
            Self::PreToolUse => "PreToolUse",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
        }
    }
}

/// Discover and load all plugins from a directory.
pub fn discover_plugins(plugins_dir: &Path) -> Vec<Plugin> {
    let mut plugins = Vec::new();
    let entries = match fs::read_dir(plugins_dir) {
        Ok(e) => e,
        Err(_) => return plugins,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(plugin) = load_plugin(&path) {
            plugins.push(plugin);
        }
    }
    plugins
}

/// Load a single plugin from a directory.
pub fn load_plugin(root: &Path) -> Option<Plugin> {
    let plugin_dir = root.join(".claude-plugin");
    if !plugin_dir.is_dir() {
        return None;
    }

    let mut load_warnings = Vec::new();

    // Parse plugin.json
    let plugin_json = fs::read_to_string(plugin_dir.join("plugin.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<PluginJson>(&s).ok());
    let name = plugin_json
        .as_ref()
        .map(|j| j.name.clone())
        .unwrap_or_else(|| {
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let description = plugin_json
        .as_ref()
        .map(|j| j.description.clone())
        .unwrap_or_default();
    let version = plugin_json.as_ref().and_then(|j| j.version.clone());

    // Parse hooks.json
    let hooks_path = root.join("hooks").join("hooks.json");
    let hooks = if hooks_path.exists() {
        match parse_hooks_json(&hooks_path) {
            Ok(h) => h,
            Err(e) => {
                load_warnings.push(format!("failed to parse hooks.json: {e}"));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Parse .mcp.json
    let mcp_config = root
        .join(".mcp.json")
        .exists()
        .then(|| {
            fs::read_to_string(root.join(".mcp.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<McpConfig>(&s).ok())
        })
        .flatten();

    Some(Plugin {
        name,
        version,
        description,
        root: root.to_path_buf(),
        hooks,
        mcp_config,
        load_warnings,
    })
}

fn parse_hooks_json(path: &Path) -> Result<Vec<HookHandler>, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let parsed: HooksJson = serde_json::from_str(&raw).map_err(|e| e.to_string())?;

    let mut handlers = Vec::new();
    // Flatten matcher groups into individual handlers with event binding
    // Note: event binding is stored in RegisteredHook when we connect to runtime
    for group in parsed.stop {
        for h in group.hooks {
            handlers.push(h);
        }
    }
    for group in parsed.post_tool_use {
        for h in group.hooks {
            handlers.push(h);
        }
    }
    for group in parsed.pre_tool_use {
        for h in group.hooks {
            handlers.push(h);
        }
    }
    for group in parsed.user_prompt_submit {
        for h in group.hooks {
            handlers.push(h);
        }
    }
    for group in parsed.session_start {
        for h in group.hooks {
            handlers.push(h);
        }
    }
    for group in parsed.session_end {
        for h in group.hooks {
            handlers.push(h);
        }
    }
    Ok(handlers)
}

/// Produce all registered hooks for a plugin, binding each handler to its event.
pub fn registered_hooks_for_plugin(plugin: &Plugin) -> Vec<RegisteredHook> {
    // Re-parse hooks.json to get the event grouping
    let hooks_path = plugin.root.join("hooks").join("hooks.json");
    let parsed: Option<HooksJson> = fs::read_to_string(&hooks_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let mut registered = Vec::new();
    let Some(parsed) = parsed else {
        return registered;
    };

    for group in parsed.stop {
        for h in group.hooks {
            registered.push(RegisteredHook {
                event: HookEvent::Stop,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.post_tool_use {
        for h in group.hooks {
            registered.push(RegisteredHook {
                event: HookEvent::PostToolUse,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.pre_tool_use {
        for h in group.hooks {
            registered.push(RegisteredHook {
                event: HookEvent::PreToolUse,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.user_prompt_submit {
        for h in group.hooks {
            registered.push(RegisteredHook {
                event: HookEvent::UserPromptSubmit,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.session_start {
        for h in group.hooks {
            registered.push(RegisteredHook {
                event: HookEvent::SessionStart,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.session_end {
        for h in group.hooks {
            registered.push(RegisteredHook {
                event: HookEvent::SessionEnd,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    registered
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    #[test]
    fn loads_plugin_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.json"),
            json!({
                "name": "test-plugin",
                "version": "1.0.0",
                "description": "A test plugin"
            })
            .to_string(),
        )
        .unwrap();

        let hooks_dir = dir.path().join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        fs::write(
            hooks_dir.join("hooks.json"),
            json!({
                "Stop": [{
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "echo ok", "timeout": 10}]
                }]
            })
            .to_string(),
        )
        .unwrap();

        let plugin = load_plugin(dir.path()).unwrap();
        assert_eq!(plugin.name, "test-plugin");
        assert_eq!(plugin.version.unwrap(), "1.0.0");
        assert_eq!(plugin.hooks.len(), 1);

        let registered = registered_hooks_for_plugin(&plugin);
        assert_eq!(registered.len(), 1);
        assert_eq!(registered[0].event, HookEvent::Stop);
        assert_eq!(registered[0].handler.command, "echo ok");
    }
}
