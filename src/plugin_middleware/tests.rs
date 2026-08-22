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
        &BuiltinPluginConfig::default(),
    );

    assert_eq!(sources.len(), 4);
    assert_eq!(sources[0].source.label(), "builtin");
    assert_eq!(
        sources[0].plugins_dir,
        rara_home.join(builtin::BUILTIN_PLUGINS_DIR)
    );
    assert_eq!(sources[1].source.label(), "user");
    assert_eq!(sources[1].plugins_dir, rara_home.join("plugins"));
    assert_eq!(sources[2].source.label(), "project");
    assert_eq!(
        sources[2].plugins_dir,
        workspace_root.join(".rara").join("plugins")
    );
    assert_eq!(sources[3].source.label(), "cli");
    assert_eq!(sources[3].plugins_dir, explicit_plugins_dir);
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
        &BuiltinPluginConfig::default(),
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

#[test]
fn builtin_nowledge_mem_plugin_materializes_skills_mcp_and_agent() {
    let dir = tempdir().expect("tempdir");
    let rara_home = dir.path().join("home").join(".rara");
    let workspace_root = dir.path().join("workspace");

    let plugins = discover_runtime_plugins(
        Some(&rara_home),
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
    );

    let plugin = plugins
        .iter()
        .find(|plugin| plugin.name == builtin::NOWLEDGE_MEM_PLUGIN_DIR)
        .expect("builtin nowledge mem plugin");
    assert!(matches!(plugin.source, PluginSource::Builtin(_)));
    assert!(
        plugin
            .root
            .join(".codex-plugin")
            .join("plugin.json")
            .is_file()
    );
    assert!(plugin.root.join(".mcp.json").is_file());
    assert!(
        plugin
            .root
            .join("skills")
            .join("working-memory")
            .join("SKILL.md")
            .is_file()
    );

    let skill_roots = plugin_skill_roots(&plugins);
    assert!(skill_roots.iter().any(|(name, _)| name == "nowledge-mem"));

    let records = plugin_agent_records(&plugins);
    let mem_agent = records
        .iter()
        .find(|record| record.id == "nowledge-mem:nowledge-mem")
        .expect("builtin mem agent");
    assert_eq!(
        mem_agent
            .definition
            .as_ref()
            .expect("definition")
            .description,
        "Routes memory-heavy tasks through Nowledge Mem skills and MCP context."
    );
}

#[test]
fn disabled_builtin_nowledge_mem_plugin_is_not_discovered() {
    let dir = tempdir().expect("tempdir");
    let rara_home = dir.path().join("home").join(".rara");
    let workspace_root = dir.path().join("workspace");
    let config = BuiltinPluginConfig {
        nowledge_mem: crate::config::NowledgeMemPluginConfig {
            enabled: false,
            ..Default::default()
        },
    };

    let plugins = discover_runtime_plugins(Some(&rara_home), &workspace_root, &[], &config);

    assert!(
        plugins
            .iter()
            .all(|plugin| plugin.name != builtin::NOWLEDGE_MEM_PLUGIN_DIR)
    );
    assert!(
        !rara_home
            .join(builtin::BUILTIN_PLUGINS_DIR)
            .join(builtin::NOWLEDGE_MEM_PLUGIN_DIR)
            .exists()
    );
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
    let registered = register_plugin_hooks(
        &runtime,
        Some(rara_home),
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
        "session-1",
    )
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
    let registered = register_plugin_hooks(
        &runtime,
        Some(rara_home),
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
        "session-1",
    )
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
    append_plugin_mcp_configs(
        &mut registry,
        None,
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
    )
    .expect("append plugin mcp config");

    let server = registry.servers.get("docs").expect("docs server");
    assert_eq!(server.source.scope, McpServerScope::Plugin);
    assert_eq!(server.source.path, plugin_root.join(".mcp.json"));
    assert_eq!(
        server.config.transport,
        McpServerTransport::Stdio {
            r#type: None,
            command: "docs-server".to_string(),
            args: vec!["--stdio".to_string()],
            env: None,
            cwd: Some(plugin_root),
        }
    );
}

#[test]
fn appends_builtin_nowledge_mem_mcp_config() {
    let dir = tempdir().expect("tempdir");
    let rara_home = dir.path().join("home").join(".rara");
    let workspace_root = dir.path().join("workspace");

    let mut registry = McpRegistry::empty();
    append_plugin_mcp_configs(
        &mut registry,
        Some(&rara_home),
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
    )
    .expect("append builtin plugin mcp config");

    let server = registry
        .servers
        .get("nowledge-mem")
        .expect("nowledge mem server");
    assert_eq!(server.source.scope, McpServerScope::Builtin);
    assert_eq!(
        server.config.transport,
        McpServerTransport::StreamableHttp {
            r#type: Some("http".to_string()),
            url: builtin::NOWLEDGE_MEM_MCP_URL.to_string(),
            bearer_token_env_var: None,
            http_headers: Some(std::collections::BTreeMap::from([(
                "APP".to_string(),
                "RARA".to_string()
            )])),
            env_http_headers: None,
        }
    );
}

#[test]
fn builtin_nowledge_mem_mcp_uses_configured_url_and_headers() {
    let dir = tempdir().expect("tempdir");
    let rara_home = dir.path().join("home").join(".rara");
    let workspace_root = dir.path().join("workspace");
    let config = BuiltinPluginConfig {
        nowledge_mem: crate::config::NowledgeMemPluginConfig {
            url: "http://localhost:24242/mcp/".to_string(),
            http_headers: std::collections::BTreeMap::from([
                ("APP".to_string(), "CustomRara".to_string()),
                ("X-NMEM-Space".to_string(), "workspace".to_string()),
            ]),
            ..Default::default()
        },
    };

    let mut registry = McpRegistry::empty();
    append_plugin_mcp_configs(
        &mut registry,
        Some(&rara_home),
        &workspace_root,
        &[],
        &config,
    )
    .expect("append builtin plugin mcp config");

    let server = registry
        .servers
        .get("nowledge-mem")
        .expect("nowledge mem server");
    assert_eq!(
        server.config.transport,
        McpServerTransport::StreamableHttp {
            r#type: Some("http".to_string()),
            url: "http://localhost:24242/mcp/".to_string(),
            bearer_token_env_var: None,
            http_headers: Some(std::collections::BTreeMap::from([
                ("APP".to_string(), "CustomRara".to_string()),
                ("X-NMEM-Space".to_string(), "workspace".to_string()),
            ])),
            env_http_headers: None,
        }
    );
}

#[test]
fn builtin_nowledge_mem_mcp_supports_cloud_auth_without_persisting_secrets() {
    let dir = tempdir().expect("tempdir");
    let rara_home = dir.path().join("home").join(".rara");
    let workspace_root = dir.path().join("workspace");
    let config = BuiltinPluginConfig {
        nowledge_mem: crate::config::NowledgeMemPluginConfig {
            mode: crate::config::NowledgeMemMode::Cloud,
            url: "http://127.0.0.1:14242/mcp/".to_string(),
            api_key_env_var: "RARA_NMEM_API_KEY".to_string(),
            space_id_env_var: Some("RARA_NMEM_SPACE".to_string()),
            ..Default::default()
        },
    };

    let mut registry = McpRegistry::empty();
    append_plugin_mcp_configs(
        &mut registry,
        Some(&rara_home),
        &workspace_root,
        &[],
        &config,
    )
    .expect("append builtin plugin mcp config");

    let server = registry
        .servers
        .get("nowledge-mem")
        .expect("nowledge mem server");
    assert_eq!(
        server.config.transport,
        McpServerTransport::StreamableHttp {
            r#type: Some("http".to_string()),
            url: builtin::NOWLEDGE_MEM_CLOUD_MCP_URL.to_string(),
            bearer_token_env_var: None,
            http_headers: Some(std::collections::BTreeMap::from([(
                "APP".to_string(),
                "RARA".to_string()
            )])),
            env_http_headers: Some(std::collections::BTreeMap::from([
                ("Authorization".to_string(), "RARA_NMEM_API_KEY".to_string()),
                (
                    "X-NMEM-API-Key".to_string(),
                    "RARA_NMEM_API_KEY".to_string()
                ),
                ("X-Nmem-Space-Id".to_string(), "RARA_NMEM_SPACE".to_string())
            ])),
        }
    );

    let materialized = std::fs::read_to_string(
        rara_home
            .join(builtin::BUILTIN_PLUGINS_DIR)
            .join(builtin::NOWLEDGE_MEM_PLUGIN_DIR)
            .join(".mcp.json"),
    )
    .expect("materialized mcp config");
    assert!(materialized.contains("RARA_NMEM_API_KEY"));
    assert!(!materialized.contains("secret"));
}

#[test]
fn builtin_nowledge_mem_mcp_yields_to_existing_registry_server() {
    let dir = tempdir().expect("tempdir");
    let rara_home = dir.path().join("home").join(".rara");
    let workspace_root = dir.path().join("workspace");

    let mut registry = McpRegistry::empty();
    registry
        .insert_source(
            McpServerSource {
                scope: McpServerScope::User,
                path: dir.path().join("config.toml"),
            },
            std::collections::BTreeMap::from([(
                "nowledge-mem".to_string(),
                McpServerConfig {
                    transport: McpServerTransport::StreamableHttp {
                        r#type: None,
                        url: "https://mem.example.com/mcp/".to_string(),
                        bearer_token_env_var: Some("NMEM_TOKEN".to_string()),
                        http_headers: None,
                        env_http_headers: None,
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
        .expect("insert user source");

    append_plugin_mcp_configs(
        &mut registry,
        Some(&rara_home),
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
    )
    .expect("append builtin plugin mcp config");

    assert_eq!(registry.servers.len(), 1);
    assert_eq!(
        registry.servers["nowledge-mem"].source.scope,
        McpServerScope::User
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
    append_plugin_mcp_configs(
        &mut registry,
        None,
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
    )
    .expect("append plugin mcp config");

    let server = registry.servers.get("docs").expect("docs server");
    assert_eq!(
        server.config.transport,
        McpServerTransport::Stdio {
            r#type: None,
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
    append_plugin_mcp_configs(
        &mut registry,
        None,
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
    )
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
                        r#type: None,
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

    let err = append_plugin_mcp_configs(
        &mut registry,
        None,
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
    )
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
    let err = append_plugin_mcp_configs(
        &mut registry,
        None,
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
    )
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
    register_plugin_hooks(
        &runtime,
        Some(rara_home),
        &workspace_root,
        &[],
        &BuiltinPluginConfig::default(),
        "session-1",
    )
    .await;

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
        call_id: "call-1".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({}),
    };
    let edit_event = AgentEvent::ToolUse {
        call_id: "call-2".to_string(),
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
        call_id: "call-1".to_string(),
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
