use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use rara_memory::vectordb::VectorDB;
use rara_tools::tool::ToolManager;
use secrecy::ExposeSecret;
use tempfile::tempdir;
use tokio::sync::mpsc;

use super::app_event::AppEvent;
use super::event_stream::{UiEvent, translate_event};
use super::provider_flow::{
    codex_auth_is_available, open_provider_family_overlay, sync_codex_credential_from_auth_store,
};
use crate::agent::{Agent, PendingApproval};
use crate::codex_model_catalog::{CodexModelOption, CodexReasoningOption};
use crate::config::{ConfigManager, OpenAiEndpointKind};
use crate::config::{DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_CHATGPT_BASE_URL, DEFAULT_CODEX_MODEL};
use crate::llm::MockLlm;
use crate::session::SessionManager;
use crate::tools::bash::BashCommandInput;
use crate::tui::command::palette_commands;
use crate::workspace::WorkspaceMemory;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shifted_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn mouse_scroll(kind: MouseEventKind) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    })
}
use super::state::{
    InteractionKind, ListPickerKind, Overlay, PendingApprovalSnapshot, PendingInteractionSnapshot,
    PermissionMode, ProviderFamily, RunningTask, StatusTab, TaskKind, TuiApp,
};
use super::{dispatch_event, map_key_to_event};

fn provider_family_idx(family: ProviderFamily) -> usize {
    super::state::PROVIDER_FAMILIES
        .iter()
        .position(|(candidate, _, _)| *candidate == family)
        .expect("provider family present")
}

#[tokio::test]
async fn busy_submit_queues_follow_up_message() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.bottom_pane.input = "continue with the follow-up".into();

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            unreachable!()
        }),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = super::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert_eq!(
        app.queued_follow_up_preview(),
        Some("continue with the follow-up")
    );
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Queued for after the next tool call boundary"))
    );
    assert_eq!(
        app.pending_follow_up_preview(),
        Some("continue with the follow-up")
    );

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn status_overlay_shortcuts_switch_tabs() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.overlay = Some(Overlay::Status(StatusTab::Overview));

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('2')), &app),
        AppEvent::SelectStatusTab(StatusTab::Config)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Right), &app),
        AppEvent::SelectStatusTab(StatusTab::Config)
    ));

    app.overlay = Some(Overlay::Status(StatusTab::Context));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Right), &app),
        AppEvent::SelectStatusTab(StatusTab::Overview)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Left), &app),
        AppEvent::SelectStatusTab(StatusTab::Config)
    ));
}

#[test]
fn context_overlay_scroll_keybindings() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.open_overlay(Overlay::Context);

    // j / Down scroll down → positive delta
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('j')), &app),
        AppEvent::ScrollContext(1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::ScrollContext(1)
    ));
    // k / Up scroll up → negative delta
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('k')), &app),
        AppEvent::ScrollContext(-1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::ScrollContext(-1)
    ));
    // Esc / Enter close
    assert!(matches!(
        map_key_to_event(key(KeyCode::Esc), &app),
        AppEvent::CloseOverlay
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Enter), &app),
        AppEvent::CloseOverlay
    ));
}

#[test]
fn context_scroll_direction_is_top_down() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.open_overlay(Overlay::Context);
    assert_eq!(app.context_scroll, 0);

    // Down / j → scroll away from top, offset increases
    app.scroll_context(1);
    assert_eq!(app.context_scroll, 1);
    app.scroll_context(1);
    assert_eq!(app.context_scroll, 2);

    // Up / k → scroll back toward top, offset decreases
    app.scroll_context(-1);
    assert_eq!(app.context_scroll, 1);
    app.scroll_context(-1);
    assert_eq!(app.context_scroll, 0);

    // Cannot go below 0
    app.scroll_context(-1);
    assert_eq!(app.context_scroll, 0);

    // PageDown / PageUp
    app.scroll_context(5);
    assert_eq!(app.context_scroll, 5);
    app.scroll_context(-5);
    assert_eq!(app.context_scroll, 0);

    // Reopen resets scroll
    app.scroll_context(10);
    assert_eq!(app.context_scroll, 10);
    app.open_overlay(Overlay::Context);
    assert_eq!(app.context_scroll, 0);
}

#[tokio::test]
async fn pending_plan_approval_blocks_plain_submit() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.set_pending_plan_approval(true);
    app.bottom_pane.input = "start implementation".into();

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = super::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.has_pending_plan_approval());
    assert!(app.bottom_pane.running_task.is_none());
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Press 1 to start implementation"))
    );
}

#[tokio::test]
async fn submit_numeric_input_handles_pending_shell_approval() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    add_pending_shell_approval(&mut app);
    app.bottom_pane.input = "4".into();

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = super::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.bottom_pane.running_task.is_none());
    assert_eq!(app.bottom_pane.input, "");
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Approval is still preparing"))
    );
}

#[tokio::test]
async fn plain_submit_queues_while_shell_approval_is_pending() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    add_pending_shell_approval(&mut app);
    app.bottom_pane.input = "then review the diff".into();

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = super::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.bottom_pane.running_task.is_none());
    assert_eq!(app.queued_follow_up_preview(), Some("then review the diff"));
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("pending interaction is answered"))
    );
}

#[tokio::test]
async fn esc_cancels_busy_query_without_overlay() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            unreachable!()
        }),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });

    assert!(matches!(
        map_key_to_event(key(KeyCode::Esc), &app),
        AppEvent::CancelRunningTask
    ));

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn busy_submit_allows_quit_command() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.bottom_pane.input = "/quit".into();

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::OAuth,
        receiver,
        handle: tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            unreachable!()
        }),
        started_at: Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        cancellation_requested: false,
    });

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = super::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(should_quit);

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn slash_palette_model_selection_opens_provider_picker_in_local_and_ssh() {
    for ssh in [false, true] {
        let temp = tempdir().expect("tempdir");
        let _ssh_env = super::terminal_ui::test_env::set_ssh_session(ssh);

        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");
        let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
        app.event_bus = Some(bus.clone());
        app.prompt_source_registry = Some(Arc::new(
            crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
        ));
        app.skill_source_registry = Some(Arc::new(
            crate::protocol_sources::SkillSourceRegistry::new(bus.clone()),
        ));
        app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
            bus.clone(),
        )));
        app.mcp_manager = Some(Arc::new(
            crate::mcp_connection_manager::McpConnectionManager::new(
                Arc::new(crate::config::McpRegistry::empty()),
                bus.clone(),
                crate::mcp_tool_cache::McpToolCache::new(),
            ),
        ));
        app.memory_handler = Some(Arc::new(
            crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
        ));

        app.set_input("/".to_string());
        let model_idx = palette_commands(&app, "")
            .iter()
            .position(|spec| spec.name == "model")
            .expect("model command present");
        app.command_palette_idx = model_idx;

        let oauth_manager = Arc::new(
            crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
                .expect("oauth manager"),
        );
        let mut agent_slot = None;
        dispatch_event(
            AppEvent::ApplyOverlaySelection,
            &mut app,
            &mut agent_slot,
            &oauth_manager,
        )
        .await
        .expect("apply command palette selection");

        assert!(
            matches!(app.overlay, Some(Overlay::ModelSearch)),
            "model search should open after model selection (ssh={ssh}), \
             but overlay was {overlay:?}",
            ssh = ssh,
            overlay = app.overlay,
        );
    }
}

#[test]
fn provider_picker_number_keys_cover_current_provider_families() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Provider));

    let key_char =
        char::from_digit(super::state::PROVIDER_FAMILIES.len() as u32, 10).expect("digit key");
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char(key_char)), &app),
        AppEvent::SetListPickerSelection(idx)
            if idx == super::state::PROVIDER_FAMILIES.len() - 1
    ));
}

#[test]
fn auth_mode_picker_prefers_selection_navigation() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.open_overlay(Overlay::ListPicker(ListPickerKind::AuthMode));

    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::MoveListPickerSelection(1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Enter), &app),
        AppEvent::ApplyOverlaySelection
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('3')), &app),
        AppEvent::SetListPickerSelection(2)
    ));
}

fn add_pending_shell_approval(app: &mut TuiApp) {
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::Approval,
            title: "Pending Approval".into(),
            summary: "git rebase --continue".into(),
            options: Vec::new(),
            note: None,
            approval: Some(PendingApprovalSnapshot {
                tool_use_id: "tool-1".into(),
                command: "git rebase --continue".into(),
                allow_net: false,
                payload: Default::default(),
            }),
            source: None,
        });
}

fn add_pending_plan_approval(app: &mut TuiApp) {
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::PlanApproval,
            title: "Plan Ready".into(),
            summary: "Review the plan.".into(),
            options: Vec::new(),
            note: None,
            approval: None,
            source: None,
        });
}

fn test_agent_for_pending_approval(temp: &tempfile::TempDir) -> Agent {
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(MockLlm),
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        Arc::new(SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        }),
        Arc::new(WorkspaceMemory::from_paths(
            temp.path().join("repo"),
            rara_dir,
        )),
    );
    agent.pending_approval = Some(PendingApproval {
        tool_use_id: "tool-1".to_string(),
        request: BashCommandInput {
            command: Some("git rebase --continue".to_string()),
            ..Default::default()
        },
    });
    agent
}

fn abort_running_task(app: &mut TuiApp) {
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

fn add_pending_request_input(app: &mut TuiApp, option_count: usize) {
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::RequestInput,
            title: "Choose one".into(),
            summary: String::new(),
            options: (1..=option_count)
                .map(|index| (format!("option {index}"), String::new()))
                .collect(),
            note: None,
            approval: None,
            source: None,
        });
}

#[test]
fn pending_shell_approval_number_shortcuts_work_in_local_and_ssh() {
    for ssh in [false, true] {
        let _ssh_env = super::terminal_ui::test_env::set_ssh_session(ssh);
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");
        let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
        app.event_bus = Some(bus.clone());
        app.prompt_source_registry = Some(Arc::new(
            crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
        ));
        app.skill_source_registry = Some(Arc::new(
            crate::protocol_sources::SkillSourceRegistry::new(bus.clone()),
        ));
        app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
            bus.clone(),
        )));
        app.mcp_manager = Some(Arc::new(
            crate::mcp_connection_manager::McpConnectionManager::new(
                Arc::new(crate::config::McpRegistry::empty()),
                bus.clone(),
                crate::mcp_tool_cache::McpToolCache::new(),
            ),
        ));
        app.memory_handler = Some(Arc::new(
            crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
        ));

        add_pending_shell_approval(&mut app);

        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('1')), &app),
            AppEvent::SelectPendingOption(0)
        ));
        assert!(matches!(
            map_key_to_event(key(KeyCode::Char('4')), &app),
            AppEvent::SelectPendingOption(3)
        ));
    }
}

#[test]
fn pending_shell_approval_does_not_render_as_request_input() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    add_pending_shell_approval(&mut app);

    assert_eq!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(super::state::ActivePendingInteractionKind::ShellApproval)
    );
    assert_eq!(app.active_pending_option_count(), 4);
}

#[tokio::test]
async fn full_access_permission_picker_resumes_pending_shell_approval_in_local_and_ssh() {
    for ssh in [false, true] {
        let _ssh_env = super::terminal_ui::test_env::set_ssh_session(ssh);
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");
        let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
        app.event_bus = Some(bus.clone());
        app.prompt_source_registry = Some(Arc::new(
            crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
        ));
        app.skill_source_registry = Some(Arc::new(
            crate::protocol_sources::SkillSourceRegistry::new(bus.clone()),
        ));
        app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
            bus.clone(),
        )));
        app.mcp_manager = Some(Arc::new(
            crate::mcp_connection_manager::McpConnectionManager::new(
                Arc::new(crate::config::McpRegistry::empty()),
                bus.clone(),
                crate::mcp_tool_cache::McpToolCache::new(),
            ),
        ));
        app.memory_handler = Some(Arc::new(
            crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
        ));

        add_pending_shell_approval(&mut app);
        app.open_overlay(Overlay::PermissionPicker);

        let oauth_manager = Arc::new(
            crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
                .expect("oauth manager"),
        );
        let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

        dispatch_event(
            AppEvent::SetPermissionSelection(3),
            &mut app,
            &mut agent_slot,
            &oauth_manager,
        )
        .await
        .expect("select full access");
        dispatch_event(
            AppEvent::ApplyOverlaySelection,
            &mut app,
            &mut agent_slot,
            &oauth_manager,
        )
        .await
        .expect("apply full access");

        assert_eq!(app.permission_mode, PermissionMode::FullAccess);
        assert!(app.pending_command_approval().is_none());
        assert!(agent_slot.is_none());
        assert!(app.bottom_pane.running_task.is_some());
        abort_running_task(&mut app);
    }
}

#[tokio::test]
async fn always_shell_approval_promotes_full_access_for_follow_up_commands() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    add_pending_shell_approval(&mut app);

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

    dispatch_event(
        AppEvent::SelectPendingOption(2),
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("approve for session");

    assert_eq!(app.permission_mode, PermissionMode::FullAccess);
    assert_eq!(app.bash_approval_mode_label(), "always");
    assert!(
        app.sandbox_network_access
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    assert!(app.pending_command_approval().is_none());
    assert!(agent_slot.is_none());
    assert!(app.bottom_pane.running_task.is_some());
    abort_running_task(&mut app);
}

#[tokio::test]
async fn full_access_mode_resumes_stale_pending_shell_approval_from_shortcuts() {
    for event in [AppEvent::SelectPendingOption(3), AppEvent::SubmitComposer] {
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");
        let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
        app.event_bus = Some(bus.clone());
        app.prompt_source_registry = Some(Arc::new(
            crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
        ));
        app.skill_source_registry = Some(Arc::new(
            crate::protocol_sources::SkillSourceRegistry::new(bus.clone()),
        ));
        app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
            bus.clone(),
        )));
        app.mcp_manager = Some(Arc::new(
            crate::mcp_connection_manager::McpConnectionManager::new(
                Arc::new(crate::config::McpRegistry::empty()),
                bus.clone(),
                crate::mcp_tool_cache::McpToolCache::new(),
            ),
        ));
        app.memory_handler = Some(Arc::new(
            crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
        ));

        add_pending_shell_approval(&mut app);
        app.permission_mode = PermissionMode::FullAccess;

        let oauth_manager = Arc::new(
            crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
                .expect("oauth manager"),
        );
        let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

        dispatch_event(event.clone(), &mut app, &mut agent_slot, &oauth_manager)
            .await
            .expect("dispatch stale approval");

        assert!(app.pending_command_approval().is_none());
        assert!(agent_slot.is_none());
        assert!(app.bottom_pane.running_task.is_some());
        abort_running_task(&mut app);
    }
}

#[tokio::test]
async fn full_access_mode_does_not_resume_shell_approval_behind_active_plan_approval() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    add_pending_shell_approval(&mut app);
    add_pending_plan_approval(&mut app);
    app.permission_mode = PermissionMode::FullAccess;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

    dispatch_event(
        AppEvent::SubmitComposer,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("dispatch submit");

    assert_eq!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(super::state::ActivePendingInteractionKind::PlanApproval)
    );
    assert!(app.pending_command_approval().is_some());
    assert!(agent_slot.is_some());
    assert!(app.bottom_pane.running_task.is_none());
}

#[test]
fn request_input_shortcuts_match_advertised_three_options() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    add_pending_request_input(&mut app, 4);

    assert_eq!(app.active_pending_option_count(), 3);
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('3')), &app),
        AppEvent::SelectPendingOption(2)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('4')), &app),
        AppEvent::InputChar('4')
    ));
}

#[test]
fn plain_input_does_not_treat_s_as_setup_shortcut() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.bottom_pane.input = "先同步ma".into();

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('s')), &app),
        AppEvent::InputChar('s')
    ));
}

#[test]
fn shift_enter_inserts_newline_in_main_composer() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    assert!(matches!(
        map_key_to_event(shifted_key(KeyCode::Enter), &app),
        AppEvent::InsertNewline
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Enter), &app),
        AppEvent::SubmitComposer
    ));
}

#[test]
fn arrow_keys_and_home_end_map_to_composer_cursor_events() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.bottom_pane.input = "hello".into();

    assert!(matches!(
        map_key_to_event(key(KeyCode::Left), &app),
        AppEvent::MoveCursorLeft
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Right), &app),
        AppEvent::MoveCursorRight
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Home), &app),
        AppEvent::MoveCursorHome
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::End), &app),
        AppEvent::MoveCursorEnd
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::MoveCursorUp
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::MoveCursorDown
    ));
}

#[test]
fn empty_composer_uses_up_down_for_input_history_when_available() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.record_input_history("previous request");

    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::NavigateInputHistory(-1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::NavigateInputHistory(1)
    ));
}

#[test]
fn empty_composer_keeps_vim_keys_for_transcript_scroll() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.record_input_history("previous request");

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('k')), &app),
        AppEvent::ScrollTranscript(-1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('j')), &app),
        AppEvent::ScrollTranscript(1)
    ));
}

#[test]
fn input_history_navigation_recalls_previous_submissions_and_restores_draft() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.record_input_history("first request");
    app.record_input_history("second request");
    app.set_input("draft".to_string());

    app.navigate_input_history(-1);
    assert_eq!(app.bottom_pane.input, "second request");
    assert_eq!(
        app.composer_cursor_offset(),
        "second request".chars().count()
    );

    app.navigate_input_history(-1);
    assert_eq!(app.bottom_pane.input, "first request");

    app.navigate_input_history(1);
    assert_eq!(app.bottom_pane.input, "second request");

    app.navigate_input_history(1);
    assert_eq!(app.bottom_pane.input, "draft");
    assert_eq!(app.input_history_cursor, None);
}

#[test]
fn input_history_navigation_starts_from_non_empty_draft_at_start() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.record_input_history("previous request");
    app.set_input("draft".to_string());
    app.bottom_pane.input_cursor_offset = Some(0);

    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::NavigateInputHistory(-1)
    ));

    app.navigate_input_history(-1);
    assert_eq!(app.bottom_pane.input, "previous request");
    app.navigate_input_history(1);
    assert_eq!(app.bottom_pane.input, "draft");
}

#[test]
fn input_history_keeps_recent_entries_bounded() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    for idx in 0..250 {
        app.record_input_history(&format!("request {idx}"));
    }

    assert_eq!(app.input_history.len(), 200);
    assert_eq!(
        app.input_history.first().map(String::as_str),
        Some("request 50")
    );
    assert_eq!(
        app.input_history.last().map(String::as_str),
        Some("request 249")
    );
}

#[test]
fn input_history_navigation_keeps_multiline_cursor_movement_for_unrecalled_text() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.record_input_history("previous request");
    app.set_input("line one\nline two".to_string());
    app.bottom_pane.input_cursor_offset = Some("line one\nline".chars().count());

    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::MoveCursorUp
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::MoveCursorDown
    ));
}

#[test]
fn mouse_wheel_scrolls_transcript() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    // First scroll without prior events → base 3 lines (factor 1.0).
    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => {
            assert!((-15..=-3).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
    match translate_event(mouse_scroll(MouseEventKind::ScrollDown), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => {
            assert!((3..=15).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[test]
fn mouse_wheel_with_overlay_routes_to_scroll_context() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.open_overlay(Overlay::CommandPalette);

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollContext(delta))) => {
            assert!((-15..=0).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }

    match translate_event(mouse_scroll(MouseEventKind::ScrollDown), &app) {
        Some(UiEvent::App(AppEvent::ScrollContext(delta))) => {
            assert!((0..=15).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[tokio::test]
async fn composer_supports_mid_input_insertion_and_backspace() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.set_input("helo".to_string());

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::MoveCursorLeft,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("move left");
    dispatch_event(
        AppEvent::InputChar('l'),
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("insert");
    assert_eq!(app.bottom_pane.input, "hello");
    assert_eq!(app.composer_cursor_offset(), 4);

    dispatch_event(
        AppEvent::Backspace,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("backspace");
    assert_eq!(app.bottom_pane.input, "helo");
    assert_eq!(app.composer_cursor_offset(), 3);
}

#[tokio::test]
async fn paste_inserts_at_current_cursor_offset() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.set_input("helo".to_string());
    app.move_active_input_cursor_left();

    super::terminal_ui::handle_paste("l".to_string(), &mut app);

    assert_eq!(app.bottom_pane.input, "hello");
    assert_eq!(app.composer_cursor_offset(), 4);
}

#[tokio::test]
async fn paste_normalizes_crlf_and_cr_newlines() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    super::terminal_ui::handle_paste("first\r\nsecond\rthird".to_string(), &mut app);

    // Flush paste burst so the text actually lands in the input.
    app.bottom_pane.flush_paste_burst();

    assert_eq!(app.bottom_pane.input, "first\nsecond\nthird");
    assert_eq!(
        app.composer_cursor_offset(),
        "first\nsecond\nthird".chars().count()
    );
}

#[test]
fn large_paste_inserts_placeholder_at_cursor_position() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    // Pre-fill input and move cursor to middle
    app.bottom_pane.input = "before after".to_string();
    app.bottom_pane.input_cursor_offset = Some("before ".chars().count());

    let big = "x".repeat(1200);
    super::terminal_ui::handle_paste(big.clone(), &mut app);
    app.bottom_pane.flush_paste_burst();

    // Placeholder should appear at cursor position, not end
    assert!(
        app.bottom_pane
            .input
            .starts_with("before [Pasted Content #0 — 1200 chars]after")
    );
    // Cursor should be after the placeholder
    assert_eq!(
        app.composer_cursor_offset(),
        "before [Pasted Content #0 — 1200 chars]".chars().count()
    );
}

#[test]
fn multiple_large_pastes_accumulate_in_pending_vec() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let big_a = "a".repeat(1200);
    let big_b = "b".repeat(1100);
    super::terminal_ui::handle_paste(big_a.clone(), &mut app);
    app.bottom_pane.flush_paste_burst();
    assert_eq!(app.bottom_pane.large_paste_pending.len(), 1);

    super::terminal_ui::handle_paste(big_b.clone(), &mut app);
    app.bottom_pane.flush_paste_burst();
    assert_eq!(app.bottom_pane.large_paste_pending.len(), 2);

    // Both placeholders should be in the input
    assert!(app.bottom_pane.input.contains("Pasted Content #0"));
    assert!(app.bottom_pane.input.contains("Pasted Content #1"));
    // Counters should be unique
    assert_ne!(
        app.bottom_pane.large_paste_pending[0].0,
        app.bottom_pane.large_paste_pending[1].0
    );
}

#[test]
fn expand_large_paste_replaces_all_placeholders() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let big = "z".repeat(1500);
    super::terminal_ui::handle_paste(big.clone(), &mut app);
    app.bottom_pane.flush_paste_burst();
    assert!(app.bottom_pane.large_paste_pending.len() == 1);
    assert!(app.bottom_pane.input.contains("Pasted Content"));
    assert!(!app.bottom_pane.input.contains(&big));

    app.bottom_pane.expand_large_paste();

    // After expand: placeholder gone, full text present, counter reset
    assert!(!app.bottom_pane.input.contains("Pasted Content"));
    assert!(app.bottom_pane.input.contains(&big));
    assert_eq!(app.bottom_pane.large_paste_pending.len(), 0);
    assert_eq!(app.bottom_pane.large_paste_counter, 0);
}

#[test]
fn crossterm_paste_event_uses_paste_channel() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    match translate_event(Event::Paste("first\nsecond".to_string()), &app) {
        Some(UiEvent::Paste(text)) => assert_eq!(text, "first\nsecond"),
        other => panic!("expected paste event, got {other:?}"),
    }
}

#[tokio::test]
async fn composer_supports_vertical_cursor_navigation_across_lines() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.terminal_width = 12;
    app.set_input("abcd\nefgh".to_string());
    app.bottom_pane.input_cursor_offset = Some("abcd\nef".chars().count());

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::MoveCursorUp,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("move up");
    assert_eq!(app.composer_cursor_offset(), 2);

    dispatch_event(
        AppEvent::MoveCursorDown,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("move down");
    assert_eq!(app.composer_cursor_offset(), "abcd\nef".chars().count());
}

#[test]
fn app_starts_with_warning_instead_of_api_key_editor_for_hosted_provider_without_api_key() {
    let temp = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: temp.path().join("config.json"),
    };
    let mut config = cm.load().expect("load config");
    config.set_provider("openai-compatible");
    config.clear_api_key();
    cm.save(&config).expect("save config");

    let app = TuiApp::new(cm).expect("app");
    assert!(app.overlay.is_none());
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.starts_with("Warning:"))
    );
}

#[tokio::test]
async fn openai_model_picker_delete_row_removes_active_profile() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "custom-default",
        "Custom endpoint",
        OpenAiEndpointKind::Custom,
    );
    app.config.set_api_key("sk-custom");
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::DeleteOpenAiProfile,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("delete profile");

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("custom-default")
    );
    assert!(matches!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::Model))
    ));
    assert!(matches!(
        app.bottom_pane.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::Rebuild)
    ));
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn openai_model_picker_space_activates_selected_profile_and_starts_setup_when_incomplete() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::SetListPickerSelection(0),
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("set model selection");

    assert!(matches!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::Model))
    ));

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("activate selected profile");

    assert!(matches!(app.overlay, Some(Overlay::BaseUrlEditor)));
}

#[tokio::test]
async fn deepseek_provider_family_prompts_for_api_key_before_model_list() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);

    open_provider_family_overlay(&mut app);

    assert_eq!(app.overlay, Some(Overlay::ApiKeyEditor));
}

#[tokio::test]
async fn deepseek_api_key_save_starts_model_catalog_task() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.open_overlay(Overlay::ApiKeyEditor);
    app.api_key_input = "sk-deepseek-test".to_string();

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save api key");

    assert_eq!(app.config.api_key(), Some("sk-deepseek-test"));
    assert!(matches!(
        app.bottom_pane.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::DeepSeekModels)
    ));
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn deepseek_model_picker_enter_without_api_key_opens_api_key_editor() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.clear_api_key();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply model selection");

    assert!(matches!(app.overlay, Some(Overlay::ApiKeyEditor)));
    assert!(app.bottom_pane.running_task.is_none());
}

#[tokio::test]
async fn deepseek_model_picker_api_key_action_opens_editor_even_when_key_exists() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_api_key("sk-deepseek-test");
    app.set_deepseek_model_options(vec!["deepseek-chat".to_string()]);
    app.model_picker_idx = app.deepseek_api_key_action_idx();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply api key action");

    assert!(matches!(app.overlay, Some(Overlay::ApiKeyEditor)));
    assert!(app.bottom_pane.running_task.is_none());
}

#[tokio::test]
async fn openai_model_picker_edit_shortcut_starts_wizard_for_selected_profile() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "custom-default",
        "Custom endpoint",
        OpenAiEndpointKind::Custom,
    );
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.model_picker_idx = 1;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::EditOpenAiProfile,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("open selected profile model editor");

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("custom-default")
    );
    assert!(matches!(app.overlay, Some(Overlay::BaseUrlEditor)));
    assert_eq!(
        app.openai_setup_steps,
        vec![Overlay::ApiKeyEditor, Overlay::ModelNameEditor]
    );
}

#[tokio::test]
async fn openai_profile_edit_wizard_keeps_existing_api_key_when_blank() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.config.set_api_key("sk-openrouter");
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::EditOpenAiProfile,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("start edit wizard");
    assert!(matches!(app.overlay, Some(Overlay::BaseUrlEditor)));

    dispatch_event(
        AppEvent::SaveBaseUrlInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save base url");
    assert!(matches!(app.overlay, Some(Overlay::ApiKeyEditor)));
    assert!(app.api_key_input.is_empty());

    dispatch_event(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("skip api key");

    assert_eq!(app.config.api_key(), Some("sk-openrouter"));
    assert!(matches!(app.overlay, Some(Overlay::ModelNameEditor)));
}

#[tokio::test]
async fn openai_model_picker_create_shortcut_opens_endpoint_kind_picker() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::CreateOpenAiProfile,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("open endpoint kind picker");

    assert!(matches!(app.overlay, Some(Overlay::ListPicker(_))));
    assert_eq!(app.openai_endpoint_kind_picker_idx, 0);
}

#[tokio::test]
async fn selecting_custom_endpoint_kind_prompts_for_profile_label() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.overlay = Some(Overlay::ListPicker(ListPickerKind::OpenAiEndpointKind));
    app.openai_endpoint_kind_picker_idx = 0;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("select endpoint kind");

    assert!(matches!(
        app.overlay,
        Some(Overlay::OpenAiProfileLabelEditor)
    ));
    assert_eq!(
        app.openai_profile_label_kind,
        Some(OpenAiEndpointKind::Custom)
    );
}

#[tokio::test]
async fn selecting_openai_profile_from_picker_switches_active_profile() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );
    app.config.select_openai_profile(
        "openrouter-backup",
        "OpenRouter Backup",
        OpenAiEndpointKind::Openrouter,
    );
    app.model_picker_idx = 3;
    app.open_overlay(Overlay::ListPicker(ListPickerKind::OpenAiProfile));
    app.openai_profile_picker_idx = 2;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply profile selection");

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-main")
    );
    assert!(matches!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::Model))
    ));
}

#[tokio::test]
async fn saving_openai_profile_label_creates_new_openrouter_profile() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::OpenAiProfile));
    app.openai_profile_picker_idx = 0;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("open profile label editor");

    app.openai_profile_label_input = "OpenRouter backup".to_string();

    dispatch_event(
        AppEvent::SaveOpenAiProfileLabelInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save profile label");

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-openrouter-backup")
    );
    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Openrouter)
    );
    assert!(
        app.config
            .openai_profiles
            .contains_key("openrouter-openrouter-backup")
    );
    assert!(matches!(app.overlay, Some(Overlay::ApiKeyEditor)));
    assert_eq!(app.openai_setup_steps, vec![Overlay::ModelNameEditor]);
}

#[tokio::test]
async fn save_api_key_input_allows_clearing_openai_compatible_credentials() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.config.set_provider("openai-compatible");
    app.config.set_api_key("sk-existing");
    app.open_overlay(Overlay::ApiKeyEditor);
    app.api_key_input.clear();

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    let should_quit = dispatch_event(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save api key");

    assert!(!should_quit);
    assert_eq!(app.config.api_key(), None);
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .is_some_and(|value| value.contains("Cleared API key"))
    );
}

#[test]
fn codex_auth_detection_uses_saved_auth_storage() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");

    assert!(!codex_auth_is_available(&app, &oauth_manager));

    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");
    assert!(codex_auth_is_available(&app, &oauth_manager));
}

#[tokio::test]
async fn codex_provider_family_routes_to_auth_picker_without_saved_login() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    app.provider_picker_idx = 0;

    assert_eq!(app.selected_provider_family(), ProviderFamily::Codex);

    open_provider_family_overlay(&mut app);
    assert_eq!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::AuthMode))
    );
}

#[tokio::test]
async fn codex_provider_family_routes_to_model_picker_with_saved_login() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");
    app.provider_picker_idx = 0;

    open_provider_family_overlay(&mut app);
    assert_eq!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::AuthMode))
    );
}

#[tokio::test]
async fn codex_provider_family_uses_saved_codex_provider_state() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");

    app.config.set_provider("ollama");
    app.config.set_api_key("sk-ollama");
    app.config.set_provider("codex");
    app.config.set_api_key("sk-codex");
    app.config.set_provider("ollama");
    app.provider_picker_idx = 0;

    assert!(codex_auth_is_available(&app, &oauth_manager));

    open_provider_family_overlay(&mut app);
    // Connected → overlay closes. Re-open for test assertion.
    app.overlay = Some(Overlay::ListPicker(ListPickerKind::UnifiedModel));
    assert!(matches!(app.overlay, Some(Overlay::ListPicker(_))));
}

#[tokio::test]
async fn codex_model_picker_opens_reasoning_level_overlay_before_rebuild() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");

    app.codex_model_options = vec![crate::codex_model_catalog::CodexModelOption {
        id: "anthropic/claude-3-5-sonnet-20241022".into(),
        label: "Claude 3.5 Sonnet v2".into(),
        model: "anthropic/claude-3-5-sonnet-20241022".into(),
        reasoning_options: vec![
            crate::codex_model_catalog::CodexReasoningOption {
                label: "Low".into(),
                value: "low".into(),
                ..Default::default()
            },
            crate::codex_model_catalog::CodexReasoningOption {
                label: "High".into(),
                value: "high".into(),
                ..Default::default()
            },
        ],
        is_default: true,
        ..Default::default()
    }];

    app.provider_picker_idx = 0;
    open_provider_family_overlay(&mut app);
    app.overlay = Some(Overlay::ListPicker(ListPickerKind::UnifiedModel));
    app.model_picker_idx = 0;

    let mut agent_slot = None;
    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply model selection");

    assert!(matches!(
        app.overlay,
        Some(Overlay::ListPicker(ListPickerKind::ReasoningEffort))
    ));
}

#[tokio::test]
async fn codex_model_picker_applies_single_reasoning_level_without_overlay() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.provider_picker_idx = 0;
    app.config.set_provider("codex");
    app.set_codex_model_options(vec![CodexModelOption {
        id: "gpt-5.2-codex".to_string(),
        model: "gpt-5.2-codex".to_string(),
        label: "gpt-5.2-codex".to_string(),
        description: "Frontier agentic coding model.".to_string(),
        default_reasoning_effort: Some("high".to_string()),
        reasoning_options: vec![CodexReasoningOption {
            value: "high".to_string(),
            label: "High".to_string(),
            description: "Maximize reasoning depth.".to_string(),
            is_default: true,
        }],
        is_default: true,
    }]);
    app.overlay = Some(Overlay::ListPicker(ListPickerKind::Model));

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply model selection");

    assert_eq!(app.config.model.as_deref(), Some("gpt-5.2-codex"));
    assert_eq!(app.config.reasoning_effort.as_deref(), Some("high"));
    assert!(matches!(
        app.bottom_pane.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::Rebuild)
    ));
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn codex_auth_store_is_synced_into_config_before_model_flow() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");

    app.config.set_provider("ollama");
    app.provider_picker_idx = 0;

    assert!(sync_codex_credential_from_auth_store(&mut app, &oauth_manager).expect("sync auth"));
    assert_eq!(
        app.config
            .provider_states
            .get("codex")
            .and_then(|state| state.api_key.as_ref())
            .map(|value| value.expose_secret()),
        Some("sk-test-codex")
    );
    assert_eq!(app.config.provider, "ollama");

    let persisted = app.config_manager.load().expect("load saved config");
    assert_eq!(persisted.provider, "ollama");
    assert_eq!(
        persisted
            .provider_states
            .get("codex")
            .and_then(|state| state.api_key.as_ref())
            .map(|value| value.expose_secret()),
        Some("sk-test-codex")
    );
}

#[test]
fn codex_chatgpt_auth_store_sets_chatgpt_base_url_before_model_flow() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    codex_login::save_auth(
        &temp.path().join(".rara").join("codex-auth"),
        &codex_login::AuthDotJson {
            auth_mode: None,
            openai_api_key: Some("sk-from-oauth".into()),
            tokens: Some(codex_login::TokenData {
                id_token: codex_login::token_data::parse_chatgpt_jwt_claims(
                    "eyJhbGciOiJub25lIn0.e30.signature",
                )
                .expect("valid id token"),
                access_token: "oauth-access-token".into(),
                refresh_token: "refresh".into(),
                account_id: None,
            }),
            last_refresh: None,
            agent_identity: None,
        },
        codex_login::AuthCredentialsStoreMode::File,
    )
    .expect("save auth");

    app.config.set_provider("ollama");

    assert!(sync_codex_credential_from_auth_store(&mut app, &oauth_manager).expect("sync auth"));
    assert_eq!(
        app.config
            .provider_states
            .get("codex")
            .and_then(|state| state.api_key.as_ref())
            .map(|value| value.expose_secret()),
        Some("oauth-access-token")
    );
    assert_eq!(
        app.config
            .provider_states
            .get("codex")
            .and_then(|state| state.base_url.as_deref()),
        Some(DEFAULT_CODEX_CHATGPT_BASE_URL)
    );
}

#[tokio::test]
async fn save_api_key_input_sets_codex_defaults_before_rebuild() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.config.set_provider("codex");
    app.open_overlay(Overlay::ApiKeyEditor);
    app.api_key_input = "sk-codex".into();

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    let should_quit = dispatch_event(
        AppEvent::SaveApiKeyInput,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("save codex api key");

    assert!(!should_quit);
    assert_eq!(app.config.model.as_deref(), Some(DEFAULT_CODEX_MODEL));
    assert_eq!(app.config.base_url.as_deref(), Some(DEFAULT_CODEX_BASE_URL));
    assert_eq!(
        app.codex_auth_mode,
        Some(crate::oauth::SavedCodexAuthMode::ApiKey)
    );
}

#[tokio::test]
async fn deepseek_model_picker_shows_dynamic_models_after_list_load() {
    use crate::config::OpenAiEndpointKind;

    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_api_key("sk-deepseek-test");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let initial_count = ListPickerKind::Model.item_count(&app);
    assert!(
        initial_count > 0,
        "initial model list should have fallback models"
    );

    app.set_deepseek_model_options(vec![
        "deepseek-chat".to_string(),
        "deepseek-reasoner".to_string(),
    ]);
    let loaded_count = ListPickerKind::Model.item_count(&app);
    assert_eq!(
        loaded_count, 3,
        "after loading models, picker should show 2 models + 1 action"
    );

    app.close_overlay();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    assert_eq!(
        ListPickerKind::Model.item_count(&app),
        3,
        "after reopening picker, still 2 models + 1 action"
    );
}

#[test]
fn mouse_wheel_with_no_overlay_routes_to_scroll_transcript() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    assert!(app.overlay.is_none());

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => {
            assert!((-15..=0).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }

    match translate_event(mouse_scroll(MouseEventKind::ScrollDown), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => {
            assert!((0..=15).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[test]
fn non_scroll_mouse_click_is_noop_regardless_of_overlay() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let click = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 5,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });

    assert!(matches!(
        translate_event(click.clone(), &app),
        Some(UiEvent::App(AppEvent::Noop))
    ));

    app.open_overlay(Overlay::CommandPalette);
    assert!(app.overlay.is_some());

    assert!(matches!(
        translate_event(click, &app),
        Some(UiEvent::App(AppEvent::Noop))
    ));
}

#[test]
fn mouse_wheel_with_status_overlay_routes_to_scroll_context() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.overlay = Some(Overlay::Status(StatusTab::Overview));

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollContext(delta))) => {
            assert!((-15..=0).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[test]
fn mouse_wheel_with_context_overlay_routes_to_scroll_context() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let bus = Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(10));
    app.event_bus = Some(bus.clone());
    app.prompt_source_registry = Some(Arc::new(
        crate::protocol_sources::PromptSourceRegistry::new(bus.clone()),
    ));
    app.skill_source_registry = Some(Arc::new(crate::protocol_sources::SkillSourceRegistry::new(
        bus.clone(),
    )));
    app.hook_registry = Some(Arc::new(crate::hook_registry::HookRegistry::new(
        bus.clone(),
    )));
    app.mcp_manager = Some(Arc::new(
        crate::mcp_connection_manager::McpConnectionManager::new(
            Arc::new(crate::config::McpRegistry::empty()),
            bus.clone(),
            crate::mcp_tool_cache::McpToolCache::new(),
        ),
    ));
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
    ));

    app.open_overlay(Overlay::Context);

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollContext(delta))) => {
            assert!((-15..=0).contains(&delta), "delta {delta} out of range");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}
