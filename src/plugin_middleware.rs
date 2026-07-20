//! Bridge between `rara-plugins` and RARA's `HookRuntime`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rara_plugins::{
    HookEvent, HookInput, PluginDiscoverySource, PluginSource, discover_plugins_from_sources,
    execute_command_hook,
};

use crate::agent::AgentEvent;
use crate::hook_runtime::HookRuntime;
use crate::runtime_control::HookLifecycle;

fn agent_event_to_hook_event(event: &AgentEvent) -> Option<HookEvent> {
    match event {
        AgentEvent::AgentStop { .. } => Some(HookEvent::Stop),
        AgentEvent::ToolUse { .. } => Some(HookEvent::PreToolUse),
        AgentEvent::ToolResult { .. } => Some(HookEvent::PostToolUse),
        _ => None,
    }
}

fn hook_event_to_lifecycle(event: HookEvent) -> HookLifecycle {
    match event {
        HookEvent::Stop => HookLifecycle::Stop,
        HookEvent::PreToolUse => HookLifecycle::PreToolUse,
        HookEvent::PostToolUse => HookLifecycle::PostToolUse,
        HookEvent::UserPromptSubmit => HookLifecycle::UserPromptSubmit,
        HookEvent::SessionStart => HookLifecycle::SessionStart,
        HookEvent::SessionEnd => HookLifecycle::SessionEnd,
    }
}

pub async fn register_plugin_hooks(
    runtime: &Arc<HookRuntime>,
    rara_home: Option<PathBuf>,
    workspace_root: &Path,
    explicit_plugin_dirs: &[PathBuf],
    session_id: &str,
) -> usize {
    let runtime = runtime.clone();
    let workspace_root = workspace_root.to_path_buf();
    let explicit_plugin_dirs = explicit_plugin_dirs.to_vec();
    let session_id = session_id.to_string();
    match tokio::task::spawn_blocking(move || {
        let resolved_rara_home = rara_home.or_else(|| crate::config::ensure_rara_home_dir().ok());
        let sources = plugin_discovery_sources(
            resolved_rara_home.as_deref(),
            &workspace_root,
            &explicit_plugin_dirs,
        );
        register_plugin_hooks_blocking(&runtime, sources, &session_id)
    })
    .await
    {
        Ok(count) => count,
        Err(err) => {
            eprintln!("plugin hook registration task failed: {err}");
            0
        }
    }
}

fn plugin_discovery_sources(
    rara_home: Option<&Path>,
    workspace_root: &Path,
    explicit_plugin_dirs: &[PathBuf],
) -> Vec<PluginDiscoverySource> {
    let project_plugins_dir = workspace_root.join(".rara").join("plugins");
    let mut sources = Vec::new();
    if let Some(rara_home) = rara_home {
        let user_plugins_dir = rara_home.join("plugins");
        sources.push(PluginDiscoverySource {
            plugins_dir: user_plugins_dir.clone(),
            source: PluginSource::User(user_plugins_dir),
        });
    }
    sources.push(PluginDiscoverySource {
        plugins_dir: project_plugins_dir.clone(),
        source: PluginSource::Project(project_plugins_dir),
    });
    sources.extend(
        explicit_plugin_dirs
            .iter()
            .cloned()
            .map(|plugins_dir| PluginDiscoverySource {
                plugins_dir: plugins_dir.clone(),
                source: PluginSource::Cli(plugins_dir),
            }),
    );
    sources
}

fn register_plugin_hooks_blocking(
    runtime: &Arc<HookRuntime>,
    sources: Vec<PluginDiscoverySource>,
    session_id: &str,
) -> usize {
    let plugins = discover_plugins_from_sources(&sources);
    let mut registered = 0usize;

    for plugin in &plugins {
        let registered_hooks = rara_plugins::loader::registered_hooks_for_plugin(plugin);
        for rh in &registered_hooks {
            let hook = rh.handler.clone();
            let plugin_name = rh.plugin_name.clone();
            let plugin_root = rh.plugin_root.clone();
            let session_id = session_id.to_string();
            let lifecycle = hook_event_to_lifecycle(rh.event);

            let plugin_name_for_callback = plugin_name.clone();
            let runtime_for_output = runtime.clone();
            let callback = Box::new(move |event: &AgentEvent| {
                if !hook_matches_agent_event(&hook, event) {
                    return;
                }
                let hook_event_name = match agent_event_to_hook_event(event) {
                    Some(e) => e.as_str().to_string(),
                    None => return,
                };

                let input = HookInput {
                    session_id: session_id.clone(),
                    transcript_path: None,
                    hook_event: hook_event_name,
                    plugin_root: plugin_root.to_string_lossy().to_string(),
                    tool_name: extract_tool_name(event),
                    tool_input: None,
                };

                let h = hook.clone();
                let pr = plugin_root.clone();
                let pn = plugin_name_for_callback.clone();
                let r = runtime_for_output.clone();
                tokio::task::spawn(async move {
                    let result = execute_command_hook(&h, &pr, input).await;
                    if !result.ok {
                        eprintln!(
                            "plugin hook {pn} failed: {} / {}",
                            result.exit_code.unwrap_or(-1),
                            result.stderr
                        );
                    }
                    if !result.stdout.trim().is_empty() {
                        r.push_output(result.stdout);
                    }
                });
            });

            runtime.register(
                format!("{}-{}", plugin_name, rh.event.as_str()),
                lifecycle,
                callback,
            );

            registered += 1;
        }
    }

    registered
}

fn extract_tool_name(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::ToolUse { name, .. } => Some(name.clone()),
        AgentEvent::ToolResult { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn hook_matches_agent_event(hook: &rara_plugins::HookHandler, event: &AgentEvent) -> bool {
    let Some(matcher) = hook.matcher.as_deref() else {
        return true;
    };
    let matcher = matcher.trim();
    if matcher.is_empty() || matcher == "*" {
        return true;
    }
    let Some(tool_name) = extract_tool_name(event) else {
        return true;
    };
    tool_name_matches(matcher, &tool_name)
}

fn tool_name_matches(matcher: &str, tool_name: &str) -> bool {
    let tool_name = tool_name.trim();
    matcher
        .split(['|', ','])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .any(|part| {
            let tool_pattern = part
                .split_once('(')
                .map(|(name, _)| name)
                .unwrap_or(part)
                .trim();
            tool_pattern == "*" || tool_pattern.eq_ignore_ascii_case(tool_name)
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::runtime_event_bus::RuntimeEventBus;

    #[test]
    fn plugin_discovery_sources_order_user_project_then_cli() {
        let dir = tempdir().expect("tempdir");
        let rara_home = dir.path().join("home").join(".rara");
        let workspace_root = dir.path().join("workspace");
        let explicit_plugins_dir = dir.path().join("explicit-plugins");

        let sources = plugin_discovery_sources(
            Some(&rara_home),
            &workspace_root,
            std::slice::from_ref(&explicit_plugins_dir),
        );

        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].source.label(), "user");
        assert_eq!(sources[0].plugins_dir, rara_home.join("plugins"));
        assert_eq!(sources[1].source.label(), "project");
        assert_eq!(
            sources[1].plugins_dir,
            workspace_root.join(".rara").join("plugins")
        );
        assert_eq!(sources[2].source.label(), "cli");
        assert_eq!(sources[2].plugins_dir, explicit_plugins_dir);
    }

    #[test]
    fn plugin_discovery_sources_keep_project_when_user_home_is_unavailable() {
        let dir = tempdir().expect("tempdir");
        let workspace_root = dir.path().join("workspace");
        let explicit_plugins_dir = dir.path().join("explicit-plugins");

        let sources = plugin_discovery_sources(
            None,
            &workspace_root,
            std::slice::from_ref(&explicit_plugins_dir),
        );

        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].source.label(), "project");
        assert_eq!(
            sources[0].plugins_dir,
            workspace_root.join(".rara").join("plugins")
        );
        assert_eq!(sources[1].source.label(), "cli");
        assert_eq!(sources[1].plugins_dir, explicit_plugins_dir);
    }

    #[tokio::test]
    async fn registers_user_and_project_plugins_with_project_precedence() {
        let dir = tempdir().expect("tempdir");
        let rara_home = dir.path().join("home").join(".rara");
        let workspace_root = dir.path().join("workspace");
        let user_plugins_dir = rara_home.join("plugins");
        let project_plugins_dir = workspace_root.join(".rara").join("plugins");
        write_test_plugin(&user_plugins_dir.join("shared"), "shared", "echo user");
        write_test_plugin(
            &project_plugins_dir.join("shared"),
            "shared",
            "echo project",
        );
        write_test_plugin(
            &project_plugins_dir.join("project-only"),
            "project-only",
            "echo project-only",
        );

        let runtime = Arc::new(HookRuntime::new(Arc::new(RuntimeEventBus::new(4))));
        let registered =
            register_plugin_hooks(&runtime, Some(rara_home), &workspace_root, &[], "session-1")
                .await;

        assert_eq!(registered, 2);
        assert_eq!(runtime.hook_count(), 2);
    }

    #[test]
    fn hook_matcher_filters_tool_events_by_tool_name() {
        let bash_hook = rara_plugins::HookHandler {
            r#type: "command".to_string(),
            command: "echo bash".to_string(),
            timeout: 1,
            matcher: Some("Bash(*)".to_string()),
            once: false,
        };
        let edit_hook = rara_plugins::HookHandler {
            r#type: "command".to_string(),
            command: "echo edit".to_string(),
            timeout: 1,
            matcher: Some("Write|Edit".to_string()),
            once: false,
        };
        let bash_event = AgentEvent::ToolUse {
            name: "bash".to_string(),
            input: serde_json::json!({}),
        };
        let edit_event = AgentEvent::ToolUse {
            name: "Edit".to_string(),
            input: serde_json::json!({}),
        };

        assert!(hook_matches_agent_event(&bash_hook, &bash_event));
        assert!(!hook_matches_agent_event(&bash_hook, &edit_event));
        assert!(hook_matches_agent_event(&edit_hook, &edit_event));
        assert!(!hook_matches_agent_event(&edit_hook, &bash_event));
    }

    fn write_test_plugin(root: &Path, name: &str, command: &str) {
        fs::create_dir_all(root.join(".claude-plugin")).expect("metadata dir");
        fs::write(
            root.join(".claude-plugin").join("plugin.json"),
            json!({
                "name": name,
                "version": "1.0.0",
                "description": "test plugin"
            })
            .to_string(),
        )
        .expect("plugin json");
        fs::create_dir_all(root.join("hooks")).expect("hooks dir");
        fs::write(
            root.join("hooks").join("hooks.json"),
            json!({
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": command,
                        "timeout": 1
                    }]
                }]
            })
            .to_string(),
        )
        .expect("hooks json");
    }
}
