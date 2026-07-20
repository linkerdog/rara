//! Bridge between `rara-plugins` and RARA's `HookRuntime`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rara_plugins::{
    HookEvent, HookInput, PluginDiscoverySource, PluginSource, RegisteredHook,
    discover_plugins_from_sources, execute_command_hook,
};
use serde_json::Value;

use crate::agent::AgentEvent;
use crate::hook_runtime::HookRuntime;
use crate::runtime_control::HookLifecycle;

#[derive(Clone, Debug, Default)]
pub(crate) struct PluginHookRuntime {
    session_id: String,
    hooks: Vec<RegisteredHook>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginHookBlock {
    pub plugin_name: String,
    pub message: String,
}

impl PluginHookRuntime {
    fn new(session_id: String, hooks: Vec<RegisteredHook>) -> Self {
        Self { session_id, hooks }
    }

    #[cfg(test)]
    fn hook_count(&self) -> usize {
        self.hooks.len()
    }

    pub(crate) async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
    ) -> Option<PluginHookBlock> {
        for hook in self.matching_hooks(HookEvent::PreToolUse, tool_name) {
            let input = self.hook_input(
                HookEvent::PreToolUse,
                hook,
                Some(tool_name.to_string()),
                Some(tool_input.clone()),
                None,
            );
            let result = execute_command_hook(&hook.handler, &hook.plugin_root, input).await;
            if !result.ok {
                if !result.stderr.trim().is_empty() {
                    eprintln!(
                        "plugin hook {} failed: {} / {}",
                        hook.plugin_name,
                        result.exit_code.unwrap_or(-1),
                        result.stderr
                    );
                }
                return Some(PluginHookBlock {
                    plugin_name: hook.plugin_name.clone(),
                    message: plugin_hook_block_message(&result.stdout, &result.stderr),
                });
            }
        }
        None
    }

    fn matching_hooks(
        &self,
        event: HookEvent,
        tool_name: &str,
    ) -> impl Iterator<Item = &RegisteredHook> {
        self.hooks.iter().filter(move |hook| {
            hook.event == event
                && is_command_hook(&hook.handler)
                && hook_handler_matches_tool(&hook.handler, Some(tool_name))
        })
    }

    fn hook_input(
        &self,
        event: HookEvent,
        hook: &RegisteredHook,
        tool_name: Option<String>,
        tool_input: Option<Value>,
        tool_response: Option<Value>,
    ) -> HookInput {
        HookInput {
            session_id: self.session_id.clone(),
            transcript_path: None,
            hook_event: event.as_str().to_string(),
            plugin_root: hook.plugin_root.to_string_lossy().to_string(),
            tool_name,
            tool_input,
            tool_response,
        }
    }
}

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

pub(crate) async fn register_plugin_hooks(
    runtime: &Arc<HookRuntime>,
    rara_home: Option<PathBuf>,
    workspace_root: &Path,
    explicit_plugin_dirs: &[PathBuf],
    session_id: &str,
) -> Arc<PluginHookRuntime> {
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
        let plugin_runtime = load_plugin_hooks_blocking(sources, &session_id);
        register_plugin_hooks_blocking(&runtime, &plugin_runtime);
        plugin_runtime
    })
    .await
    {
        Ok(plugin_runtime) => Arc::new(plugin_runtime),
        Err(err) => {
            eprintln!("plugin hook registration task failed: {err}");
            Arc::new(PluginHookRuntime::default())
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

fn load_plugin_hooks_blocking(
    sources: Vec<PluginDiscoverySource>,
    session_id: &str,
) -> PluginHookRuntime {
    let plugins = discover_plugins_from_sources(&sources);
    let mut hooks = Vec::new();
    for plugin in &plugins {
        hooks.extend(rara_plugins::loader::registered_hooks_for_plugin(plugin));
    }
    PluginHookRuntime::new(session_id.to_string(), hooks)
}

fn register_plugin_hooks_blocking(
    runtime: &Arc<HookRuntime>,
    plugin_runtime: &PluginHookRuntime,
) -> usize {
    let mut registered = 0usize;

    for rh in &plugin_runtime.hooks {
        if rh.event == HookEvent::PreToolUse || !is_command_hook(&rh.handler) {
            continue;
        }
        let hook = rh.handler.clone();
        let plugin_name = rh.plugin_name.clone();
        let plugin_root = rh.plugin_root.clone();
        let session_id = plugin_runtime.session_id.clone();
        let lifecycle = hook_event_to_lifecycle(rh.event);

        let plugin_name_for_callback = plugin_name.clone();
        let runtime_for_output = Arc::downgrade(runtime);
        let callback = Box::new(move |event: &AgentEvent| {
            if !hook_matches_agent_event(&hook, event) {
                return;
            }
            let Some(runtime_for_output) = runtime_for_output.upgrade() else {
                return;
            };
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
                tool_input: extract_tool_input(event),
                tool_response: extract_tool_response(event),
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

    registered
}

fn is_command_hook(hook: &rara_plugins::HookHandler) -> bool {
    hook.r#type.is_empty() || hook.r#type == "command"
}

fn extract_tool_name(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::ToolUse { name, .. } => Some(name.clone()),
        AgentEvent::ToolResult { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn extract_tool_input(event: &AgentEvent) -> Option<Value> {
    match event {
        AgentEvent::ToolUse { input, .. } => Some(input.clone()),
        _ => None,
    }
}

fn extract_tool_response(event: &AgentEvent) -> Option<Value> {
    match event {
        AgentEvent::ToolResult { content, .. } => {
            Some(serde_json::from_str(content).unwrap_or_else(|_| Value::String(content.clone())))
        }
        _ => None,
    }
}

fn hook_matches_agent_event(hook: &rara_plugins::HookHandler, event: &AgentEvent) -> bool {
    hook_handler_matches_tool(hook, extract_tool_name(event).as_deref())
}

fn hook_handler_matches_tool(hook: &rara_plugins::HookHandler, tool_name: Option<&str>) -> bool {
    let Some(matcher) = hook.matcher.as_deref() else {
        return true;
    };
    let matcher = matcher.trim();
    if matcher.is_empty() || matcher == "*" {
        return true;
    }
    let Some(tool_name) = tool_name else {
        return true;
    };
    tool_name_matches(matcher, tool_name)
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

fn plugin_hook_block_message(stdout: &str, stderr: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<Value>(stdout) {
        for key in ["stopReason", "reason", "systemMessage"] {
            if let Some(message) = parsed.get(key).and_then(Value::as_str)
                && !message.trim().is_empty()
            {
                return message.trim().to_string();
            }
        }
    }
    if !stderr.trim().is_empty() {
        return stderr.trim().to_string();
    }
    if !stdout.trim().is_empty() {
        return stdout.trim().to_string();
    }
    "blocked by plugin hook".to_string()
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

        assert_eq!(registered.hook_count(), 2);
        assert_eq!(runtime.hook_count(), 2);
    }

    #[tokio::test]
    async fn plugin_callbacks_do_not_retain_hook_runtime_strong_reference() {
        let dir = tempdir().expect("tempdir");
        let rara_home = dir.path().join("home").join(".rara");
        let workspace_root = dir.path().join("workspace");
        let project_plugins_dir = workspace_root.join(".rara").join("plugins");
        write_test_plugin(
            &project_plugins_dir.join("project-only"),
            "project-only",
            "echo project-only",
        );

        let runtime = Arc::new(HookRuntime::new(Arc::new(RuntimeEventBus::new(4))));
        register_plugin_hooks(&runtime, Some(rara_home), &workspace_root, &[], "session-1").await;

        assert_eq!(runtime.hook_count(), 1);
        assert_eq!(Arc::strong_count(&runtime), 1);
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

    #[test]
    fn extracts_tool_result_content_for_post_tool_use_hooks() {
        let event = AgentEvent::ToolResult {
            name: "stub_tool".to_string(),
            content: json!({ "status": "ok" }).to_string(),
            is_error: false,
        };

        assert_eq!(
            extract_tool_response(&event),
            Some(json!({ "status": "ok" }))
        );
    }

    #[test]
    fn plugin_hook_block_message_falls_back_to_plain_stdout() {
        assert_eq!(
            plugin_hook_block_message("plain stdout failure", ""),
            "plain stdout failure"
        );
    }

    #[tokio::test]
    async fn pre_tool_use_control_stdout_is_not_buffered_as_context() {
        let hook_runtime = HookRuntime::new(Arc::new(RuntimeEventBus::new(4)));
        let dir = tempdir().expect("tempdir");
        let plugin_root = dir.path().join("plugin");
        fs::create_dir_all(&plugin_root).expect("plugin root");
        let plugin_hooks = PluginHookRuntime::new(
            "session-1".to_string(),
            vec![rara_plugins::RegisteredHook {
                event: HookEvent::PreToolUse,
                handler: rara_plugins::HookHandler {
                    r#type: "command".to_string(),
                    command: "echo '{\"continue\":true}'".to_string(),
                    timeout: 1,
                    matcher: Some("stub_tool".to_string()),
                    once: false,
                },
                plugin_name: "control".to_string(),
                plugin_root,
            }],
        );

        let block = plugin_hooks
            .run_pre_tool_use("stub_tool", &json!({ "path": "src/lib.rs" }))
            .await;

        assert_eq!(block, None);
        assert!(hook_runtime.blocking_drain_outputs().is_empty());
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
