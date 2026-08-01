use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::types::{HookEvent, HookHandler, McpConfig, Plugin, PluginSource, RegisteredHook};

/// Directory to scan with the source metadata attached to loaded plugins.
#[derive(Debug, Clone)]
pub struct PluginDiscoverySource {
    pub plugins_dir: PathBuf,
    pub source: PluginSource,
}

/// Parsed hooks.json content.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
struct HooksJson {
    #[serde(default)]
    Stop: Vec<MatcherGroup>,
    #[serde(default)]
    PostToolUse: Vec<MatcherGroup>,
    #[serde(default)]
    PreToolUse: Vec<MatcherGroup>,
    #[serde(default)]
    UserPromptSubmit: Vec<MatcherGroup>,
    #[serde(default)]
    SessionStart: Vec<MatcherGroup>,
    #[serde(default)]
    SessionEnd: Vec<MatcherGroup>,
    #[serde(default)]
    GoalCreated: Vec<MatcherGroup>,
    #[serde(default)]
    GoalCompleted: Vec<MatcherGroup>,
}

#[derive(Debug, Clone, Deserialize)]
struct MatcherGroup {
    #[serde(default)]
    #[allow(dead_code)]
    matcher: Option<String>,
    hooks: Vec<HookHandler>,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: String,
}

/// Discover and load all plugins from a directory.
pub fn discover_plugins(plugins_dir: &Path) -> Vec<Plugin> {
    discover_plugins_from_source(
        plugins_dir,
        PluginSource::Directory(plugins_dir.to_path_buf()),
    )
}

/// Discover plugins across multiple directories and de-duplicate by plugin name.
///
/// Later sources in the slice override earlier sources with the same plugin
/// name, so callers can express their precedence rules by source ordering.
/// The returned vector is sorted by plugin name for deterministic consumers; it
/// does not preserve source or filesystem discovery order.
pub fn discover_plugins_from_sources(sources: &[PluginDiscoverySource]) -> Vec<Plugin> {
    let mut plugins_by_name = BTreeMap::new();
    for source in sources {
        for plugin in discover_plugins_from_source(&source.plugins_dir, source.source.clone()) {
            plugins_by_name.insert(plugin.name.clone(), plugin);
        }
    }
    plugins_by_name.into_values().collect()
}

/// Discover and load all plugins from one directory with explicit source metadata.
pub fn discover_plugins_from_source(plugins_dir: &Path, source: PluginSource) -> Vec<Plugin> {
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
        if let Some(plugin) = load_plugin_with_source(&path, source.clone()) {
            plugins.push(plugin);
        }
    }
    plugins
}

/// Load a single plugin from a directory.
pub fn load_plugin(root: &Path) -> Option<Plugin> {
    load_plugin_with_source(root, PluginSource::Directory(root.to_path_buf()))
}

/// Load a single plugin from a directory with explicit source metadata.
pub fn load_plugin_with_source(root: &Path, source: PluginSource) -> Option<Plugin> {
    let plugin_dir = plugin_metadata_dir(root)?;

    let mut load_warnings = Vec::new();

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
        source,
        hooks,
        mcp_config,
        load_warnings,
    })
}

fn plugin_metadata_dir(root: &Path) -> Option<PathBuf> {
    [".claude-plugin", ".codex-plugin"]
        .into_iter()
        .map(|dir| root.join(dir))
        .find(|dir| dir.is_dir())
}

fn parse_hooks_json(path: &Path) -> Result<Vec<HookHandler>, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let parsed: HooksJson = serde_json::from_str(&raw).map_err(|e| e.to_string())?;

    let mut handlers = Vec::new();
    for group in parsed.Stop {
        handlers.extend(hooks_with_group_matcher(group));
    }
    for group in parsed.PostToolUse {
        handlers.extend(hooks_with_group_matcher(group));
    }
    for group in parsed.PreToolUse {
        handlers.extend(hooks_with_group_matcher(group));
    }
    for group in parsed.UserPromptSubmit {
        handlers.extend(hooks_with_group_matcher(group));
    }
    for group in parsed.SessionStart {
        handlers.extend(hooks_with_group_matcher(group));
    }
    for group in parsed.SessionEnd {
        handlers.extend(hooks_with_group_matcher(group));
    }
    for group in parsed.GoalCreated {
        handlers.extend(hooks_with_group_matcher(group));
    }
    for group in parsed.GoalCompleted {
        handlers.extend(hooks_with_group_matcher(group));
    }
    Ok(handlers)
}

fn hooks_with_group_matcher(mut group: MatcherGroup) -> Vec<HookHandler> {
    if let Some(matcher) = group.matcher {
        for hook in &mut group.hooks {
            if hook.matcher.is_none() {
                hook.matcher = Some(matcher.clone());
            }
        }
    }
    group.hooks
}

/// Produce all registered hooks for a plugin, binding each handler to its event.
pub fn registered_hooks_for_plugin(plugin: &Plugin) -> Vec<RegisteredHook> {
    let hooks_path = plugin.root.join("hooks").join("hooks.json");
    let parsed: Option<HooksJson> = fs::read_to_string(&hooks_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let mut registered = Vec::new();
    let Some(parsed) = parsed else {
        return registered;
    };

    for group in parsed.Stop {
        for h in hooks_with_group_matcher(group) {
            registered.push(RegisteredHook {
                event: HookEvent::Stop,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.PostToolUse {
        for h in hooks_with_group_matcher(group) {
            registered.push(RegisteredHook {
                event: HookEvent::PostToolUse,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.PreToolUse {
        for h in hooks_with_group_matcher(group) {
            registered.push(RegisteredHook {
                event: HookEvent::PreToolUse,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.UserPromptSubmit {
        for h in hooks_with_group_matcher(group) {
            registered.push(RegisteredHook {
                event: HookEvent::UserPromptSubmit,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.SessionStart {
        for h in hooks_with_group_matcher(group) {
            registered.push(RegisteredHook {
                event: HookEvent::SessionStart,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.SessionEnd {
        for h in hooks_with_group_matcher(group) {
            registered.push(RegisteredHook {
                event: HookEvent::SessionEnd,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.GoalCreated {
        for h in hooks_with_group_matcher(group) {
            registered.push(RegisteredHook {
                event: HookEvent::GoalCreated,
                handler: h,
                plugin_name: plugin.name.clone(),
                plugin_root: plugin.root.clone(),
            });
        }
    }
    for group in parsed.GoalCompleted {
        for h in hooks_with_group_matcher(group) {
            registered.push(RegisteredHook {
                event: HookEvent::GoalCompleted,
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
        assert_eq!(plugin.version.as_deref(), Some("1.0.0"));
        assert_eq!(plugin.source.label(), "directory");
        assert_eq!(plugin.hooks.len(), 1);

        let registered = registered_hooks_for_plugin(&plugin);
        assert_eq!(registered.len(), 1);
        assert_eq!(registered[0].event, HookEvent::Stop);
        assert_eq!(registered[0].handler.command, "echo ok");
        assert_eq!(registered[0].handler.matcher.as_deref(), Some(""));
    }

    #[test]
    fn loads_codex_plugin_metadata_directory() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join(".codex-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.json"),
            json!({
                "name": "nowledge-mem",
                "version": "0.1.29",
                "description": "Memory that follows the user"
            })
            .to_string(),
        )
        .unwrap();

        let plugin = load_plugin(dir.path()).expect("codex plugin should load");

        assert_eq!(plugin.name, "nowledge-mem");
        assert_eq!(plugin.version.as_deref(), Some("0.1.29"));
        assert_eq!(plugin.description, "Memory that follows the user");
    }

    #[test]
    fn registered_hooks_inherit_group_matcher_unless_handler_overrides() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin").join("plugin.json"),
            json!({
                "name": "matcher-plugin",
                "description": "matcher plugin"
            })
            .to_string(),
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        fs::write(
            dir.path().join("hooks").join("hooks.json"),
            json!({
                "PreToolUse": [{
                    "matcher": "Bash(*)",
                    "hooks": [
                        {"type": "command", "command": "echo inherited"},
                        {"type": "command", "command": "echo override", "matcher": "Write|Edit"}
                    ]
                }]
            })
            .to_string(),
        )
        .unwrap();

        let plugin = load_plugin(dir.path()).unwrap();
        let registered = registered_hooks_for_plugin(&plugin);

        assert_eq!(registered.len(), 2);
        assert_eq!(registered[0].handler.matcher.as_deref(), Some("Bash(*)"));
        assert_eq!(registered[1].handler.matcher.as_deref(), Some("Write|Edit"));
    }

    #[test]
    fn registered_hooks_include_goal_lifecycle_events() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin").join("plugin.json"),
            json!({
                "name": "goal-plugin",
                "description": "goal plugin"
            })
            .to_string(),
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        fs::write(
            dir.path().join("hooks").join("hooks.json"),
            json!({
                "GoalCreated": [{
                    "hooks": [{"type": "command", "command": "echo created"}]
                }],
                "GoalCompleted": [{
                    "hooks": [{"type": "command", "command": "echo completed"}]
                }]
            })
            .to_string(),
        )
        .unwrap();

        let plugin = load_plugin(dir.path()).unwrap();
        let registered = registered_hooks_for_plugin(&plugin);
        let events = registered.iter().map(|hook| hook.event).collect::<Vec<_>>();

        assert_eq!(
            events,
            vec![HookEvent::GoalCreated, HookEvent::GoalCompleted]
        );
    }

    #[test]
    fn discovers_plugins_from_sources_with_later_source_precedence() {
        let user_dir = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let user_plugin = user_dir.path().join("shared-plugin");
        let project_plugin = project_dir.path().join("shared-plugin");
        write_test_plugin(&user_plugin, "shared-plugin", "user plugin");
        write_test_plugin(&project_plugin, "shared-plugin", "project plugin");
        write_test_plugin(
            &project_dir.path().join("project-only"),
            "project-only",
            "project only",
        );

        let plugins = discover_plugins_from_sources(&[
            PluginDiscoverySource {
                plugins_dir: user_dir.path().to_path_buf(),
                source: PluginSource::User(user_dir.path().to_path_buf()),
            },
            PluginDiscoverySource {
                plugins_dir: project_dir.path().to_path_buf(),
                source: PluginSource::Project(project_dir.path().to_path_buf()),
            },
        ]);

        assert_eq!(plugins.len(), 2);
        let shared = plugins
            .iter()
            .find(|plugin| plugin.name == "shared-plugin")
            .expect("shared plugin");
        assert_eq!(shared.description, "project plugin");
        assert_eq!(shared.source.label(), "project");
        assert!(matches!(shared.source, PluginSource::Project(_)));

        let project_only = plugins
            .iter()
            .find(|plugin| plugin.name == "project-only")
            .expect("project-only plugin");
        assert_eq!(project_only.source.label(), "project");
    }

    fn write_test_plugin(root: &Path, name: &str, description: &str) {
        let plugin_dir = root.join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.json"),
            json!({
                "name": name,
                "version": "1.0.0",
                "description": description
            })
            .to_string(),
        )
        .unwrap();
    }
}
