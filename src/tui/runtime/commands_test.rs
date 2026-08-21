use std::fs;
use std::sync::Arc;
use std::time::Instant;

use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;
use tokio::sync::mpsc;

use super::{
    execute_local_command, handle_mcp_command, handle_nowledge_mem_command,
    mcp_project_root_from_cwd, parse_goal_objective_and_budget, parse_goal_token_budget,
    spawn_mcp_tool_cache_population,
};
use crate::agent::{Agent, AgentEvent, BashApprovalMode};
use crate::config::{
    ConfigManager, McpRegistry, McpServerConfig, McpServerScope, McpServerSource,
    McpServerTransport, SourcedMcpServerConfig,
};
use crate::llm::MockLlm;
use crate::mcp_tool_cache::McpToolCache;
use crate::oauth::OAuthManager;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::session::SessionManager;
use crate::tasklist::{DEFAULT_TASK_LIST_ID, NewTaskRecord, TaskListStore};
use crate::tools::tasklist::TaskListTool;
use crate::tui::state::{
    ListPickerKind, LocalCommand, LocalCommandKind, Overlay, PermissionMode, RunningTask,
    TaskCompletion, TaskKind, TuiApp,
};
use crate::workspace::WorkspaceMemory;

fn mark_app_busy(app: &mut TuiApp) {
    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::ModelCatalog,
        receiver,
        handle: tokio::spawn(async {
            TaskCompletion::ModelCatalog {
                provider: rara_provider_catalog::ModelCatalogProvider::DeepSeek,
                result: Ok(vec![]),
            }
        }),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

fn test_agent_with_shared_task_tool(dir: &tempfile::TempDir) -> Agent {
    let rara_dir = dir.path().join(".rara");
    fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");
    let task_store = Arc::new(TaskListStore::new(rara_dir.join("tasks")));
    let mut tools = ToolManager::new();
    tools.register(Box::new(TaskListTool {
        store: task_store,
        default_task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
    }));
    Agent::new(
        tools,
        Arc::new(MockLlm),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        Arc::new(SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        }),
        Arc::new(WorkspaceMemory::from_paths(
            dir.path().join("workspace"),
            rara_dir,
        )),
    )
}

#[test]
fn mcp_project_root_walks_up_to_project_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("project");
    let nested = project.join("src").join("bin");
    fs::create_dir_all(&nested).expect("nested dirs");
    fs::write(project.join(".mcp.json"), r#"{"mcpServers":{}}"#).expect("project config");

    assert_eq!(mcp_project_root_from_cwd(nested), project);
}

#[test]
fn mcp_project_root_keeps_cwd_when_no_project_config_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path().join("project").join("src");
    fs::create_dir_all(&cwd).expect("cwd");

    assert_eq!(mcp_project_root_from_cwd(cwd.clone()), cwd);
}

#[test]
fn parses_goal_budget_tokens_like_codex_goal_command() {
    assert_eq!(parse_goal_token_budget("98.5K"), Some(98_500));
    assert_eq!(parse_goal_token_budget("2m"), Some(2_000_000));
    assert_eq!(parse_goal_token_budget("0"), None);
    assert_eq!(parse_goal_token_budget("-1"), None);
}

#[test]
fn parses_goal_objective_with_tokens_option() {
    assert_eq!(
        parse_goal_objective_and_budget("--tokens 98.5K improve benchmark coverage")
            .expect("goal command"),
        ("improve benchmark coverage".to_string(), Some(98_500))
    );
    assert_eq!(
        parse_goal_objective_and_budget("50000 fix the build").expect("legacy budget"),
        ("fix the build".to_string(), Some(50_000))
    );
    assert_eq!(
        parse_goal_objective_and_budget("fix the build").expect("no budget"),
        ("fix the build".to_string(), None)
    );
}

#[test]
fn nowledge_mem_command_opens_picker_without_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");

    handle_nowledge_mem_command(None, &mut app).expect("picker should open");
    assert_eq!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::NowledgeMem))
    );
}

#[test]
fn mcp_command_publishes_structured_status_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("config.toml"),
        r#"
[mcp_servers.docs]
command = "docs-server"
"#,
    )
    .expect("user config");
    let project = dir.path().join("project");
    fs::create_dir_all(&project).expect("project");

    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.cwd = project.to_string_lossy().to_string();
    let bus = Arc::new(RuntimeEventBus::new(8));
    let mut receiver = bus.subscribe();
    app.event_bus = Some(bus);

    handle_mcp_command(&mut app);

    let event = receiver.try_recv().expect("mcp status event");
    let AgentEvent::McpStatusUpdated(snapshot) = event else {
        panic!("expected mcp status event");
    };
    assert_eq!(snapshot.servers.len(), 1);
    assert_eq!(snapshot.servers[0].name, "docs");
}

#[test]
fn mcp_command_publishes_structured_load_failure_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("config.toml"), "[mcp_servers.docs\n").expect("user config");
    let project = dir.path().join("project");
    fs::create_dir_all(&project).expect("project");

    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.cwd = project.to_string_lossy().to_string();
    let bus = Arc::new(RuntimeEventBus::new(8));
    let mut receiver = bus.subscribe();
    app.event_bus = Some(bus);

    handle_mcp_command(&mut app);

    let event = receiver.try_recv().expect("mcp failure event");
    let AgentEvent::McpStatusLoadFailed { message } = event else {
        panic!("expected mcp load failure event");
    };
    assert!(message.contains("config.toml"));
}

#[tokio::test]
async fn mcp_tool_cache_population_clears_existing_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = McpToolCache::new();
    cache.insert_server_tools(
        "stale".to_string(),
        vec![rara_mcp_client::McpToolRecord {
            server: String::new(),
            name: "old_tool".to_string(),
            display_name: "old_tool".to_string(),
            description: "stale cached tool".to_string(),
            input_schema: serde_json::json!({}),
        }],
    );
    assert!(!cache.is_empty());

    let mut registry = McpRegistry::empty();
    registry.servers.insert(
        "http-only".to_string(),
        SourcedMcpServerConfig {
            config: McpServerConfig {
                transport: McpServerTransport::StreamableHttp {
                    r#type: None,
                    url: "http://127.0.0.1:1/mcp".to_string(),
                    bearer_token_env_var: None,
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
            source: McpServerSource {
                scope: McpServerScope::Project,
                path: dir.path().join(".mcp.json"),
            },
        },
    );

    spawn_mcp_tool_cache_population(&cache, &registry)
        .await
        .expect("cache population task");

    assert!(cache.is_empty());
}

#[tokio::test]
async fn mode_changing_commands_are_rejected_while_busy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.bash_approval_mode = BashApprovalMode::Suggestion;
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth manager"),
    );
    let mut agent_slot = None;

    mark_app_busy(&mut app);
    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Approval,
            arg: None,
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("approval command should be handled");
    assert_eq!(app.bash_approval_mode_label(), "suggestion");
    assert_eq!(app.permission_mode, PermissionMode::Auto);
    assert_eq!(
        app.bottom_pane.notice.as_deref(),
        Some("A task is already running. Wait for it to finish.")
    );

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Plan,
            arg: None,
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("plan command should be handled");
    assert_eq!(app.agent_execution_mode_label(), "execute");

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Permissions,
            arg: None,
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("permissions command should be handled");
    assert!(app.overlay.is_none());
    assert_ne!(app.overlay, Some(Overlay::PermissionPicker));
}

#[tokio::test]
async fn goal_command_refuses_to_replace_existing_goal_without_clear() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.goal = Some(crate::tui::state::RalphGoal::new(
        "existing goal".to_string(),
        None,
    ));
    *app.goal_handle.write().unwrap() = app.goal.clone();
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth manager"),
    );
    let mut agent_slot = None;

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Goal,
            arg: Some("new goal".to_string()),
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("goal command should be handled");

    assert_eq!(
        app.goal.as_ref().map(|goal| goal.objective.as_str()),
        Some("existing goal")
    );
    assert_eq!(
        app.bottom_pane.notice.as_deref(),
        Some("A goal already exists. Use /goal clear before setting a new goal.")
    );
}

#[tokio::test]
async fn dream_command_without_agent_reports_unavailable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth manager"),
    );
    let mut agent_slot = None;

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Dream,
            arg: None,
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("dream command should be handled");

    assert_eq!(
        app.bottom_pane.notice.as_deref(),
        Some("Memory consolidation is not available until an agent is ready.")
    );
}

#[tokio::test]
async fn goal_command_accepts_tokens_option() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth manager"),
    );
    let mut agent_slot = None;

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Goal,
            arg: Some("--tokens 98.5K improve benchmark coverage".to_string()),
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("goal command should be handled");

    let goal = app.goal.as_ref().expect("goal");
    assert_eq!(goal.objective, "improve benchmark coverage");
    assert_eq!(goal.token_budget, Some(98_500));
    assert_eq!(
        app.goal_handle
            .read()
            .unwrap()
            .as_ref()
            .map(|goal| goal.objective.as_str()),
        Some("improve benchmark coverage")
    );
}

#[tokio::test]
async fn goal_command_status_notice_stays_compact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    let mut goal = crate::tui::state::RalphGoal::new("finish goal polish".to_string(), Some(500));
    goal.tokens_used = 125;
    goal.turns_completed = 3;
    app.goal = Some(goal);
    *app.goal_handle.write().unwrap() = app.goal.clone();
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth manager"),
    );
    let mut agent_slot = None;

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Goal,
            arg: None,
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("goal command should be handled");

    assert_eq!(
        app.bottom_pane.notice.as_deref(),
        Some("Goal: finish goal polish [active] · 125 / 500 tokens")
    );
}

#[tokio::test]
async fn goal_command_empty_state_points_to_help() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth manager"),
    );
    let mut agent_slot = None;

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Goal,
            arg: None,
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("goal command should be handled");

    assert_eq!(
        app.bottom_pane.notice.as_deref(),
        Some("No active goal. Use /help for /goal details.")
    );
}

#[tokio::test]
async fn tasks_command_switches_agent_and_tool_default_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let task_store = TaskListStore::new(dir.path().join(".rara/tasks"));
    task_store
        .create_task(
            "team alpha",
            NewTaskRecord {
                subject: "Switch shared task list".to_string(),
                description: "Verify active task-list command.".to_string(),
                active_form: None,
                metadata: Default::default(),
            },
        )
        .expect("create task");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth manager"),
    );
    let mut agent_slot = Some(test_agent_with_shared_task_tool(&dir));
    let agent = agent_slot.as_ref().expect("agent");
    app.apply_runtime_snapshot(
        agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(agent, 0),
    );

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Tasks,
            arg: Some("team alpha".to_string()),
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("tasks command should be handled");

    let agent = agent_slot.as_ref().expect("agent");
    assert_eq!(agent.task_list_id, "team-alpha");
    assert_eq!(app.snapshot.shared_tasks.task_list_id, "team-alpha");
    assert_eq!(app.snapshot.shared_tasks.total, 1);
    let output = agent
        .tool_manager
        .get_tool("task_list")
        .expect("task_list tool")
        .call(serde_json::json!({}))
        .await
        .expect("task list");
    assert_eq!(output["tasks"][0]["subject"], "Switch shared task list");
}

#[tokio::test]
async fn approval_command_switches_always_to_full_access() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    let oauth_manager = Arc::new(
        OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth manager"),
    );
    let mut agent_slot = None;

    execute_local_command(
        LocalCommand {
            kind: LocalCommandKind::Approval,
            arg: None,
        },
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("approval command should be handled");

    assert_eq!(app.permission_mode, PermissionMode::FullAccess);
    assert_eq!(app.bash_approval_mode_label(), "always");
    assert!(
        app.sandbox_network_access
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    assert_eq!(
        app.bottom_pane.notice.as_deref(),
        Some("Permission mode: full-access.")
    );
}
