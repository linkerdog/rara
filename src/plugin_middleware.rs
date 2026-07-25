//! Bridge between `rara-plugins` and RARA's runtime registries.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use rara_plugins::{
    HookEvent, HookInput, Plugin, PluginDiscoverySource, PluginSource, RegisteredHook,
    discover_plugins_from_sources, execute_command_hook,
};
use serde_json::Value;

use crate::agent::AgentEvent;
use crate::config::{
    McpRegistry, McpServerConfig, McpServerScope, McpServerSource, McpServerTransport,
    load_mcp_servers_from_json_path,
};
use crate::hook_runtime::HookRuntime;
use crate::runtime_control::{HookEvent as RuntimeHookEvent, HookLifecycle};

#[derive(Clone, Debug, Default)]
pub(crate) struct PluginHookRuntime {
    session_id: String,
    hooks: Vec<RegisteredHook>,
    command_summaries: Vec<PluginCommandSummary>,
    hook_runtime: Option<Weak<HookRuntime>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginHookBlock {
    pub plugin_name: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginCommandSummary {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default)]
struct HookInputFields {
    tool_name: Option<String>,
    tool_input: Option<Value>,
    tool_response: Option<Value>,
    last_assistant_message: Option<String>,
    is_interrupt: Option<bool>,
    prompt: Option<String>,
}

impl PluginHookRuntime {
    fn new(
        session_id: String,
        hooks: Vec<RegisteredHook>,
        command_summaries: Vec<PluginCommandSummary>,
        hook_runtime: Option<Arc<HookRuntime>>,
    ) -> Self {
        Self {
            session_id,
            hooks,
            command_summaries,
            hook_runtime: hook_runtime.map(|runtime| Arc::downgrade(&runtime)),
        }
    }

    pub(crate) fn hook_count(&self) -> usize {
        self.hooks.len()
    }

    pub(crate) fn command_summaries(&self) -> &[PluginCommandSummary] {
        &self.command_summaries
    }

    pub(crate) async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
    ) -> Option<PluginHookBlock> {
        for hook in self.matching_hooks(HookEvent::PreToolUse, Some(tool_name)) {
            let input = self.hook_input(
                HookEvent::PreToolUse,
                hook,
                HookInputFields {
                    tool_name: Some(tool_name.to_string()),
                    tool_input: Some(tool_input.clone()),
                    ..HookInputFields::default()
                },
            );
            let result = execute_command_hook(&hook.handler, &hook.plugin_root, input).await;
            if !result.ok {
                if !result.stderr.trim().is_empty() {
                    log::warn!(
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

    pub(crate) async fn run_session_end(
        &self,
        last_assistant_message: Option<&str>,
        is_interrupt: bool,
    ) {
        for hook in self.matching_hooks(HookEvent::SessionEnd, None) {
            let input = self.hook_input(
                HookEvent::SessionEnd,
                hook,
                HookInputFields {
                    last_assistant_message: last_assistant_message.map(ToString::to_string),
                    is_interrupt: Some(is_interrupt),
                    ..HookInputFields::default()
                },
            );
            let result = execute_command_hook(&hook.handler, &hook.plugin_root, input).await;
            self.publish_hook_output(hook, HookEvent::SessionEnd, &result);
            if !result.ok {
                log::warn!(
                    "plugin hook {} failed: {} / {}",
                    hook.plugin_name,
                    result.exit_code.unwrap_or(-1),
                    result.stderr
                );
            }
        }
    }

    pub(crate) async fn run_session_start(&self) {
        self.run_unblocked_lifecycle(HookEvent::SessionStart, HookInputFields::default())
            .await;
    }

    pub(crate) async fn run_user_prompt_submit(&self, prompt: &str) {
        self.run_unblocked_lifecycle(
            HookEvent::UserPromptSubmit,
            HookInputFields {
                prompt: Some(prompt.to_string()),
                ..HookInputFields::default()
            },
        )
        .await;
    }

    async fn run_unblocked_lifecycle(&self, event: HookEvent, fields: HookInputFields) {
        for hook in self.matching_hooks(event, None) {
            let input = self.hook_input(event, hook, fields.clone());
            let result = execute_command_hook(&hook.handler, &hook.plugin_root, input).await;
            self.publish_hook_output(hook, event, &result);
            if !result.ok {
                log::warn!(
                    "plugin hook {} failed: {} / {}",
                    hook.plugin_name,
                    result.exit_code.unwrap_or(-1),
                    result.stderr
                );
            }
        }
    }

    fn publish_hook_output(
        &self,
        hook: &RegisteredHook,
        event: HookEvent,
        result: &rara_plugins::HookExecutionResult,
    ) {
        if result.stdout.trim().is_empty() && result.stderr.trim().is_empty() && result.ok {
            return;
        }
        let Some(runtime) = self.hook_runtime.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        runtime.publish_plugin_hook_output(RuntimeHookEvent::CommandOutput {
            plugin_name: hook.plugin_name.clone(),
            hook_event: event.as_str().to_string(),
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
            exit_code: result.exit_code,
            timed_out: result.timed_out,
            ok: result.ok,
        });
    }

    fn matching_hooks(
        &self,
        event: HookEvent,
        tool_name: Option<&str>,
    ) -> impl Iterator<Item = &RegisteredHook> {
        self.hooks.iter().filter(move |hook| {
            hook.event == event
                && is_command_hook(&hook.handler)
                && hook_handler_matches_tool(&hook.handler, tool_name)
        })
    }

    fn hook_input(
        &self,
        event: HookEvent,
        hook: &RegisteredHook,
        fields: HookInputFields,
    ) -> HookInput {
        HookInput {
            session_id: self.session_id.clone(),
            transcript_path: None,
            hook_event: event.as_str().to_string(),
            plugin_root: hook.plugin_root.to_string_lossy().to_string(),
            tool_name: fields.tool_name,
            tool_input: fields.tool_input,
            tool_response: fields.tool_response,
            last_assistant_message: fields.last_assistant_message,
            is_interrupt: fields.is_interrupt,
            prompt: fields.prompt,
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
        let plugin_runtime =
            load_plugin_hooks_blocking(sources, &session_id, Some(runtime.clone()));
        register_plugin_hooks_blocking(&runtime, &plugin_runtime);
        plugin_runtime
    })
    .await
    {
        Ok(plugin_runtime) => Arc::new(plugin_runtime),
        Err(err) => {
            log::warn!("plugin hook registration task failed: {err}");
            Arc::new(PluginHookRuntime::default())
        }
    }
}

pub(crate) fn append_plugin_mcp_configs(
    registry: &mut McpRegistry,
    rara_home: Option<&Path>,
    workspace_root: &Path,
    explicit_plugin_dirs: &[PathBuf],
) -> anyhow::Result<()> {
    let sources = plugin_discovery_sources(rara_home, workspace_root, explicit_plugin_dirs);
    let plugins = discover_plugins_from_sources(&sources);
    append_plugin_mcp_configs_from_plugins(registry, &plugins)
}

pub(crate) fn discover_runtime_plugins(
    rara_home: Option<&Path>,
    workspace_root: &Path,
    explicit_plugin_dirs: &[PathBuf],
) -> Vec<Plugin> {
    let sources = plugin_discovery_sources(rara_home, workspace_root, explicit_plugin_dirs);
    discover_plugins_from_sources(&sources)
}

pub(crate) fn plugin_skill_roots(plugins: &[Plugin]) -> Vec<(String, PathBuf)> {
    plugins
        .iter()
        .filter(|plugin| plugin.root.join("skills").is_dir())
        .map(|plugin| (plugin.name.clone(), plugin.root.clone()))
        .collect()
}

pub(crate) fn plugin_agent_records(
    plugins: &[Plugin],
) -> Vec<crate::tools::agent::AgentDefinitionLoadRecord> {
    let mut records = Vec::new();
    for plugin in plugins {
        let agents_dir = plugin.root.join("agents");
        let start = records.len();
        crate::tools::agent::scan_agent_records_dir(&agents_dir, &mut records);
        for record in &mut records[start..] {
            let local_id = record.id.clone();
            record.id = format!("{}:{local_id}", plugin.name);
            if let Some(definition) = &mut record.definition {
                let local_name = definition.name.clone();
                definition.name = format!("{}:{local_name}", plugin.name);
            }
        }
    }
    records
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
    runtime: Option<Arc<HookRuntime>>,
) -> PluginHookRuntime {
    let plugins = discover_plugins_from_sources(&sources);
    let mut hooks = Vec::new();
    for plugin in &plugins {
        hooks.extend(rara_plugins::loader::registered_hooks_for_plugin(plugin));
    }
    let command_summaries = plugin_command_summaries(&plugins);
    PluginHookRuntime::new(session_id.to_string(), hooks, command_summaries, runtime)
}

fn append_plugin_mcp_configs_from_plugins(
    registry: &mut McpRegistry,
    plugins: &[Plugin],
) -> anyhow::Result<()> {
    for plugin in plugins {
        let mcp_path = plugin.root.join(".mcp.json");
        if !mcp_path.is_file() {
            continue;
        }
        let mut servers = load_mcp_servers_from_json_path(&mcp_path)?;
        apply_plugin_mcp_defaults(&mut servers, &plugin.root);
        registry.insert_source(
            McpServerSource {
                scope: McpServerScope::Plugin,
                path: mcp_path,
            },
            servers,
        )?;
    }
    Ok(())
}

fn apply_plugin_mcp_defaults(
    servers: &mut std::collections::BTreeMap<String, McpServerConfig>,
    plugin_root: &Path,
) {
    for server in servers.values_mut() {
        match &mut server.transport {
            McpServerTransport::Stdio { cwd, .. } => {
                let resolved = match cwd.take() {
                    Some(path) if path.is_absolute() => path,
                    Some(path) => plugin_root.join(path),
                    None => plugin_root.to_path_buf(),
                };
                *cwd = Some(resolved);
            }
            McpServerTransport::StreamableHttp { .. } => {}
        }
    }
}

fn plugin_command_summaries(plugins: &[Plugin]) -> Vec<PluginCommandSummary> {
    let mut summaries = Vec::new();
    for plugin in plugins {
        let commands_dir = plugin.root.join("commands");
        let mut command_paths = Vec::new();
        collect_markdown_files(&commands_dir, &mut command_paths);
        for path in command_paths {
            let Some(local_name) = local_plugin_command_name(&commands_dir, &path) else {
                continue;
            };
            let content = fs::read_to_string(&path).unwrap_or_default();
            summaries.push(PluginCommandSummary {
                name: format!("{}:{local_name}", plugin.name),
                title: Some(local_name),
                description: extract_plugin_command_description(&content),
                path,
            });
        }
    }
    summaries.sort_by(|left, right| left.name.cmp(&right.name));
    summaries
}

fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

fn local_plugin_command_name(commands_dir: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(commands_dir).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let std::path::Component::Normal(raw) = component else {
            return None;
        };
        let mut part = raw.to_str()?.to_string();
        if part.ends_with(".md") {
            part.truncate(part.len() - ".md".len());
        }
        if part.is_empty() {
            return None;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn extract_plugin_command_description(content: &str) -> String {
    extract_leading_frontmatter_field(content, "description")
        .unwrap_or_else(|| extract_plugin_skill_description(content))
}

fn extract_leading_frontmatter_field(content: &str, key: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let Some((field, value)) = trimmed.split_once(':') else {
            continue;
        };
        if field.trim() != key {
            continue;
        }
        let value = trim_yaml_scalar(value);
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn trim_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        if (bytes[0] == b'"' && bytes[trimmed.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[trimmed.len() - 1] == b'\'')
        {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn extract_plugin_skill_description(content: &str) -> String {
    let body = strip_leading_frontmatter(content);
    for section in body.split("\n#") {
        for line in section.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                return trimmed.to_string();
            }
        }
    }
    "No description provided.".to_string()
}

fn strip_leading_frontmatter(content: &str) -> &str {
    if !content
        .lines()
        .next()
        .is_some_and(|line| line.trim() == "---")
    {
        return content;
    }
    let mut offset = 0usize;
    for (index, line) in content.split_inclusive('\n').enumerate() {
        offset += line.len();
        if index == 0 {
            continue;
        }
        if line.trim() == "---" {
            return &content[offset..];
        }
    }
    content
}

fn register_plugin_hooks_blocking(
    runtime: &Arc<HookRuntime>,
    plugin_runtime: &PluginHookRuntime,
) -> usize {
    let mut registered = 0usize;

    for rh in &plugin_runtime.hooks {
        if matches!(
            rh.event,
            HookEvent::PreToolUse
                | HookEvent::SessionStart
                | HookEvent::UserPromptSubmit
                | HookEvent::SessionEnd
        ) || !is_command_hook(&rh.handler)
        {
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
                last_assistant_message: None,
                is_interrupt: None,
                prompt: None,
            };

            let h = hook.clone();
            let pr = plugin_root.clone();
            let pn = plugin_name_for_callback.clone();
            let r = runtime_for_output.clone();
            tokio::task::spawn(async move {
                let result = execute_command_hook(&h, &pr, input).await;
                if !result.ok {
                    log::warn!(
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
    async fn registers_project_plugin_command_summaries() {
        let dir = tempdir().expect("tempdir");
        let rara_home = dir.path().join("home").join(".rara");
        let workspace_root = dir.path().join("workspace");
        let project_plugins_dir = workspace_root.join(".rara").join("plugins");
        let plugin_root = project_plugins_dir.join("commands");
        write_test_plugin(&plugin_root, "commands", "echo project");
        fs::create_dir_all(plugin_root.join("commands").join("git")).expect("commands dir");
        fs::write(
            plugin_root.join("commands").join("git").join("review.md"),
            "---\ndescription: Review the current diff.\n---\n# Review\nBody text.",
        )
        .expect("command");
        fs::write(
            plugin_root.join("commands").join("explain.md"),
            "# Explain\nExplain selected code.",
        )
        .expect("command");

        let runtime = Arc::new(HookRuntime::new(Arc::new(RuntimeEventBus::new(4))));
        let registered =
            register_plugin_hooks(&runtime, Some(rara_home), &workspace_root, &[], "session-1")
                .await;

        assert_eq!(
            registered.command_summaries(),
            &[
                PluginCommandSummary {
                    name: "commands:explain".to_string(),
                    title: Some("explain".to_string()),
                    description: "Explain selected code.".to_string(),
                    path: plugin_root.join("commands").join("explain.md"),
                },
                PluginCommandSummary {
                    name: "commands:git/review".to_string(),
                    title: Some("git/review".to_string()),
                    description: "Review the current diff.".to_string(),
                    path: plugin_root.join("commands").join("git").join("review.md"),
                },
            ]
        );
    }

    #[test]
    fn plugin_agent_records_are_namespaced_by_plugin_name() {
        let dir = tempdir().expect("tempdir");
        let plugin_root = dir.path().join("plugin");
        write_test_plugin(&plugin_root, "helpers", "echo project");
        fs::create_dir_all(plugin_root.join("agents")).expect("agents dir");
        fs::write(
            plugin_root.join("agents").join("reviewer.md"),
            r#"---
name: reviewer
description: Reviews plugin-provided code.
---

Review code from the plugin context.
"#,
        )
        .expect("agent");
        let plugins = discover_plugins_from_sources(&[PluginDiscoverySource {
            plugins_dir: dir.path().to_path_buf(),
            source: PluginSource::Directory(dir.path().to_path_buf()),
        }]);

        let records = plugin_agent_records(&plugins);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "helpers:reviewer");
        let definition = records[0].definition.as_ref().expect("definition");
        assert_eq!(definition.name, "helpers:reviewer");
        assert_eq!(definition.description, "Reviews plugin-provided code.");
    }

    #[test]
    fn appends_plugin_mcp_configs_with_plugin_source_metadata() {
        let dir = tempdir().expect("tempdir");
        let workspace_root = dir.path().join("workspace");
        let project_plugins_dir = workspace_root.join(".rara").join("plugins");
        let plugin_root = project_plugins_dir.join("mcp-plugin");
        write_test_plugin(&plugin_root, "mcp-plugin", "echo project");
        write_test_plugin_mcp_config(&plugin_root, "docs", "docs-server", &["--stdio"]);

        let mut registry = McpRegistry::empty();
        append_plugin_mcp_configs(&mut registry, None, &workspace_root, &[])
            .expect("append plugin mcp config");

        let server = registry.servers.get("docs").expect("docs server");
        assert_eq!(server.source.scope, McpServerScope::Plugin);
        assert_eq!(server.source.path, plugin_root.join(".mcp.json"));
        assert_eq!(
            server.config.transport,
            McpServerTransport::Stdio {
                command: "docs-server".to_string(),
                args: vec!["--stdio".to_string()],
                env: None,
                cwd: Some(plugin_root),
            }
        );
    }

    #[test]
    fn plugin_mcp_configs_resolve_relative_cwd_from_plugin_root() {
        let dir = tempdir().expect("tempdir");
        let workspace_root = dir.path().join("workspace");
        let project_plugins_dir = workspace_root.join(".rara").join("plugins");
        let plugin_root = project_plugins_dir.join("mcp-plugin");
        write_test_plugin(&plugin_root, "mcp-plugin", "echo project");
        fs::write(
            plugin_root.join(".mcp.json"),
            json!({
                "mcpServers": {
                    "docs": {
                        "command": "docs-server",
                        "cwd": "server"
                    }
                }
            })
            .to_string(),
        )
        .expect("mcp json");

        let mut registry = McpRegistry::empty();
        append_plugin_mcp_configs(&mut registry, None, &workspace_root, &[])
            .expect("append plugin mcp config");

        let server = registry.servers.get("docs").expect("docs server");
        assert_eq!(
            server.config.transport,
            McpServerTransport::Stdio {
                command: "docs-server".to_string(),
                args: Vec::new(),
                env: None,
                cwd: Some(plugin_root.join("server")),
            }
        );
    }

    #[test]
    fn plugin_mcp_configs_skip_mcp_json_directories() {
        let dir = tempdir().expect("tempdir");
        let workspace_root = dir.path().join("workspace");
        let project_plugins_dir = workspace_root.join(".rara").join("plugins");
        let plugin_root = project_plugins_dir.join("mcp-plugin");
        write_test_plugin(&plugin_root, "mcp-plugin", "echo project");
        fs::create_dir_all(plugin_root.join(".mcp.json")).expect("mcp json dir");

        let mut registry = McpRegistry::empty();
        append_plugin_mcp_configs(&mut registry, None, &workspace_root, &[])
            .expect("directory should be skipped");

        assert!(registry.servers.is_empty());
    }

    #[test]
    fn plugin_mcp_configs_fail_on_duplicate_server_names() {
        let dir = tempdir().expect("tempdir");
        let workspace_root = dir.path().join("workspace");
        let project_plugins_dir = workspace_root.join(".rara").join("plugins");
        let plugin_root = project_plugins_dir.join("mcp-plugin");
        write_test_plugin(&plugin_root, "mcp-plugin", "echo project");
        write_test_plugin_mcp_config(&plugin_root, "docs", "docs-server", &[]);

        let mut registry = McpRegistry::empty();
        registry
            .insert_source(
                McpServerSource {
                    scope: McpServerScope::Project,
                    path: workspace_root.join(".mcp.json"),
                },
                std::collections::BTreeMap::from([(
                    "docs".to_string(),
                    McpServerConfig {
                        transport: McpServerTransport::Stdio {
                            command: "project-docs".to_string(),
                            args: Vec::new(),
                            env: None,
                            cwd: None,
                        },
                        enabled: true,
                        required: false,
                        supports_parallel_tool_calls: false,
                        startup_timeout_sec: None,
                        tool_timeout_sec: None,
                        enabled_tools: None,
                        disabled_tools: None,
                    },
                )]),
            )
            .expect("insert project source");

        let err = append_plugin_mcp_configs(&mut registry, None, &workspace_root, &[])
            .expect_err("duplicate server should fail");

        let message = err.to_string();
        assert!(message.contains("MCP server `docs` is defined in both project"));
        assert!(message.contains("plugin"));
    }

    #[test]
    fn plugin_mcp_configs_fail_on_invalid_json() {
        let dir = tempdir().expect("tempdir");
        let workspace_root = dir.path().join("workspace");
        let project_plugins_dir = workspace_root.join(".rara").join("plugins");
        let plugin_root = project_plugins_dir.join("mcp-plugin");
        write_test_plugin(&plugin_root, "mcp-plugin", "echo project");
        fs::write(plugin_root.join(".mcp.json"), "{invalid json").expect("mcp json");

        let mut registry = McpRegistry::empty();
        let err = append_plugin_mcp_configs(&mut registry, None, &workspace_root, &[])
            .expect_err("invalid plugin mcp json should fail");

        let message = err.to_string();
        assert!(message.contains("parse"));
        assert!(message.contains(".mcp.json"));
    }

    #[test]
    fn plugin_skill_description_uses_first_markdown_body_section() {
        let content = "---\nname: reviewer\ndescription: metadata\n---\n# Reviewer\nInspect plugin-provided behavior.\n\n## Details\nMore.";

        assert_eq!(
            extract_plugin_skill_description(content),
            "Inspect plugin-provided behavior."
        );
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
                    command: "cat >/dev/null; echo '{\"continue\":true}'".to_string(),
                    timeout: 1,
                    matcher: Some("stub_tool".to_string()),
                    once: false,
                },
                plugin_name: "control".to_string(),
                plugin_root,
            }],
            Vec::new(),
            None,
        );

        let block = plugin_hooks
            .run_pre_tool_use("stub_tool", &json!({ "path": "src/lib.rs" }))
            .await;

        assert_eq!(block, None);
        assert!(hook_runtime.blocking_drain_outputs().is_empty());
    }

    #[tokio::test]
    async fn lifecycle_hook_output_is_published_as_structured_control_event() {
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut events = bus.subscribe_control();
        let hook_runtime = Arc::new(HookRuntime::new(bus));
        let dir = tempdir().expect("tempdir");
        let plugin_root = dir.path().join("plugin");
        fs::create_dir_all(&plugin_root).expect("plugin root");
        let plugin_hooks = PluginHookRuntime::new(
            "session-1".to_string(),
            vec![rara_plugins::RegisteredHook {
                event: HookEvent::SessionStart,
                handler: rara_plugins::HookHandler {
                    r#type: "command".to_string(),
                    command: "cat >/dev/null; echo visible; echo warn >&2".to_string(),
                    timeout: 1,
                    matcher: None,
                    once: false,
                },
                plugin_name: "observer".to_string(),
                plugin_root,
            }],
            Vec::new(),
            Some(hook_runtime.clone()),
        );

        plugin_hooks.run_session_start().await;

        let event = events.try_recv().expect("hook output event");
        assert_eq!(
            event.event,
            crate::runtime_control::RuntimeEvent::Hook(
                crate::runtime_control::HookEvent::CommandOutput {
                    plugin_name: "observer".to_string(),
                    hook_event: "SessionStart".to_string(),
                    stdout: "visible\n".to_string(),
                    stderr: "warn\n".to_string(),
                    exit_code: Some(0),
                    timed_out: false,
                    ok: true,
                }
            )
        );
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

    fn write_test_plugin_mcp_config(root: &Path, name: &str, command: &str, args: &[&str]) {
        fs::write(
            root.join(".mcp.json"),
            json!({
                "mcpServers": {
                    name: {
                        "command": command,
                        "args": args
                    }
                }
            })
            .to_string(),
        )
        .expect("mcp json");
    }
}
