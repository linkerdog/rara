//! Bridge between `rara-plugins` and RARA's `HookRuntime`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rara_plugins::{HookEvent, HookInput, discover_plugins, execute_command_hook};

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
    user_plugins_dir: &Path,
    session_id: &str,
) -> usize {
    let runtime = runtime.clone();
    let user_plugins_dir = user_plugins_dir.to_path_buf();
    let session_id = session_id.to_string();
    match tokio::task::spawn_blocking(move || {
        register_plugin_hooks_blocking(&runtime, &user_plugins_dir, &session_id)
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

fn register_plugin_hooks_blocking(
    runtime: &Arc<HookRuntime>,
    user_plugins_dir: &Path,
    session_id: &str,
) -> usize {
    let plugins = discover_plugins(user_plugins_dir);
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
