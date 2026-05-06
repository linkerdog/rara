use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
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
use crate::tool::ToolManager;
use crate::tools::bash::BashCommandInput;
use crate::tui::command::palette_commands;
use crate::vectordb::VectorDB;
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
    InteractionKind, Overlay, PendingApprovalSnapshot, PendingInteractionSnapshot, PermissionMode,
    ProviderFamily, RunningTask, StatusTab, TaskKind, TuiApp,
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
    .expect("app");
    app.input = "continue with the follow-up".into();

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.running_task = Some(RunningTask {
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
        app.notice
            .as_deref()
            .is_some_and(|value| value.contains("Queued for after the next tool call boundary"))
    );
    assert_eq!(
        app.pending_follow_up_preview(),
        Some("continue with the follow-up")
    );

    if let Some(task) = app.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn status_overlay_shortcuts_switch_tabs() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
    .expect("app");
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
    .expect("app");
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
    .expect("app");
    app.set_pending_plan_approval(true);
    app.input = "start implementation".into();

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
    assert!(app.running_task.is_none());
    assert!(
        app.notice
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
    .expect("app");
    add_pending_shell_approval(&mut app);
    app.input = "4".into();

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = super::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.running_task.is_none());
    assert_eq!(app.input, "");
    assert!(
        app.notice
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
    .expect("app");
    add_pending_shell_approval(&mut app);
    app.input = "then review the diff".into();

    let mut agent_slot = None;
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let should_quit = super::handle_submit(&mut app, &mut agent_slot, &oauth_manager)
        .await
        .expect("submit");

    assert!(!should_quit);
    assert!(app.running_task.is_none());
    assert_eq!(app.queued_follow_up_preview(), Some("then review the diff"));
    assert!(
        app.notice
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
    .expect("app");

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.running_task = Some(RunningTask {
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

    if let Some(task) = app.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn busy_submit_allows_quit_command() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.input = "/quit".into();

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.running_task = Some(RunningTask {
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

    if let Some(task) = app.running_task.take() {
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
        .expect("app");
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

        assert!(matches!(app.overlay, Some(Overlay::ListPicker(_))));
        assert_eq!(app.notice.as_deref(), Some("Opened provider picker."));
    }
}

#[test]
fn provider_picker_number_keys_cover_current_provider_families() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.open_overlay(Overlay::ProviderPicker);

    let key_char =
        char::from_digit(super::state::PROVIDER_FAMILIES.len() as u32, 10).expect("digit key");
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char(key_char)), &app),
        AppEvent::SetProviderSelection(idx)
            if idx == super::state::PROVIDER_FAMILIES.len() - 1
    ));
}

#[test]
fn auth_mode_picker_prefers_selection_navigation() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.open_overlay(Overlay::AuthModePicker);

    assert!(matches!(
        map_key_to_event(key(KeyCode::Down), &app),
        AppEvent::MoveAuthModeSelection(1)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Enter), &app),
        AppEvent::ApplyOverlaySelection
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('3')), &app),
        AppEvent::SetAuthModeSelection(2)
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
        .expect("app");
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
    .expect("app");
    add_pending_shell_approval(&mut app);

    assert_eq!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(super::state::ActivePendingInteractionKind::ShellApproval)
    );
    assert_eq!(app.active_pending_option_count(), 4);
}

#[tokio::test]
async fn full_access_permission_selection_resumes_pending_shell_approval_once() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    add_pending_shell_approval(&mut app);
    app.open_overlay(Overlay::PermissionPicker);
    app.permission_picker_idx = 3;

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = Some(test_agent_for_pending_approval(&temp));

    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply full access permission selection");

    assert_eq!(app.permission_mode, PermissionMode::FullAccess);
    assert!(app.pending_command_approval().is_none());
    assert!(agent_slot.is_none());
    assert!(app.running_task.is_some());
    if let Some(task) = app.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn request_input_shortcuts_match_advertised_three_options() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
    .expect("app");
    app.input = "先同步ma".into();

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('s')), &app),
        AppEvent::InputChar('s')
    ));
}

#[test]
fn shift_enter_inserts_newline_in_main_composer() {
    let temp = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

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
    .expect("app");
    app.input = "hello".into();

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
    .expect("app");
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
    .expect("app");
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
    .expect("app");
    app.record_input_history("first request");
    app.record_input_history("second request");
    app.set_input("draft".to_string());

    app.navigate_input_history(-1);
    assert_eq!(app.input, "second request");
    assert_eq!(
        app.composer_cursor_offset(),
        "second request".chars().count()
    );

    app.navigate_input_history(-1);
    assert_eq!(app.input, "first request");

    app.navigate_input_history(1);
    assert_eq!(app.input, "second request");

    app.navigate_input_history(1);
    assert_eq!(app.input, "draft");
    assert_eq!(app.input_history_cursor, None);
}

#[test]
fn input_history_navigation_starts_from_non_empty_draft_at_start() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.record_input_history("previous request");
    app.set_input("draft".to_string());
    app.input_cursor_offset = Some(0);

    assert!(matches!(
        map_key_to_event(key(KeyCode::Up), &app),
        AppEvent::NavigateInputHistory(-1)
    ));

    app.navigate_input_history(-1);
    assert_eq!(app.input, "previous request");
    app.navigate_input_history(1);
    assert_eq!(app.input, "draft");
}

#[test]
fn input_history_keeps_recent_entries_bounded() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

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
    .expect("app");
    app.record_input_history("previous request");
    app.set_input("line one\nline two".to_string());
    app.input_cursor_offset = Some("line one\nline".chars().count());

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
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => assert_eq!(delta, -3),
        event => panic!("unexpected event: {event:?}"),
    }
    match translate_event(mouse_scroll(MouseEventKind::ScrollDown), &app) {
        Some(UiEvent::App(AppEvent::ScrollTranscript(delta))) => assert_eq!(delta, 3),
        event => panic!("unexpected event: {event:?}"),
    }
}

#[test]
fn mouse_wheel_does_not_scroll_transcript_behind_overlay() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.open_overlay(Overlay::CommandPalette);

    match translate_event(mouse_scroll(MouseEventKind::ScrollUp), &app) {
        Some(UiEvent::App(AppEvent::Noop)) => {}
        event => panic!("unexpected event: {event:?}"),
    }
}

#[tokio::test]
async fn composer_supports_mid_input_insertion_and_backspace() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
    assert_eq!(app.input, "hello");
    assert_eq!(app.composer_cursor_offset(), 4);

    dispatch_event(
        AppEvent::Backspace,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("backspace");
    assert_eq!(app.input, "helo");
    assert_eq!(app.composer_cursor_offset(), 3);
}

#[tokio::test]
async fn paste_inserts_at_current_cursor_offset() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.set_input("helo".to_string());
    app.move_active_input_cursor_left();

    super::terminal_ui::handle_paste("l".to_string(), &mut app);

    assert_eq!(app.input, "hello");
    assert_eq!(app.composer_cursor_offset(), 4);
}

#[tokio::test]
async fn paste_normalizes_crlf_and_cr_newlines() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    super::terminal_ui::handle_paste("first\r\nsecond\rthird".to_string(), &mut app);

    assert_eq!(app.input, "first\nsecond\nthird");
    assert_eq!(
        app.composer_cursor_offset(),
        "first\nsecond\nthird".chars().count()
    );
}

#[test]
fn crossterm_paste_event_uses_paste_channel() {
    let temp = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

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
    .expect("app");
    app.terminal_width = 12;
    app.set_input("abcd\nefgh".to_string());
    app.input_cursor_offset = Some("abcd\nef".chars().count());

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
        app.notice
            .as_deref()
            .is_some_and(|value| value.starts_with("Warning:"))
    );
}

#[test]
fn openai_compatible_model_picker_exposes_profile_table_shortcuts() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ModelPicker);

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('c')), &app),
        AppEvent::CreateOpenAiProfile
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('e')), &app),
        AppEvent::EditOpenAiProfile
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char(' ')), &app),
        AppEvent::ApplyOverlaySelection
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('d')), &app),
        AppEvent::DeleteOpenAiProfile
    ));
}

#[tokio::test]
async fn openai_model_picker_delete_row_removes_active_profile() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
    app.open_overlay(Overlay::ModelPicker);

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
    assert!(matches!(app.overlay, Some(Overlay::ModelPicker)));
    assert!(matches!(
        app.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::Rebuild)
    ));
    if let Some(task) = app.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn openai_model_picker_space_activates_selected_profile_and_starts_setup_when_incomplete() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ModelPicker);

    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    let mut agent_slot = None;

    dispatch_event(
        AppEvent::SetModelSelection(0),
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("set model selection");

    assert!(matches!(app.overlay, Some(Overlay::ModelPicker)));

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
    .expect("app");
    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);

    open_provider_family_overlay(&mut app, &oauth_manager)
        .await
        .expect("open overlay");

    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Deepseek)
    );
    assert!(matches!(app.overlay, Some(Overlay::ApiKeyEditor)));
}

#[tokio::test]
async fn deepseek_api_key_save_starts_model_catalog_task() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
        app.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::DeepSeekModels)
    ));
    if let Some(task) = app.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn deepseek_model_picker_enter_without_api_key_opens_api_key_editor() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.clear_api_key();
    app.open_overlay(Overlay::ModelPicker);

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
    assert!(app.running_task.is_none());
}

#[tokio::test]
async fn deepseek_model_picker_api_key_action_opens_editor_even_when_key_exists() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_api_key("sk-deepseek-test");
    app.set_deepseek_model_options(vec!["deepseek-chat".to_string()]);
    app.model_picker_idx = app.deepseek_api_key_action_idx();
    app.open_overlay(Overlay::ModelPicker);

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
    assert!(app.running_task.is_none());
}

#[test]
fn deepseek_model_picker_accepts_uppercase_api_key_and_refresh_shortcuts() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.open_overlay(Overlay::ModelPicker);

    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('A')), &app),
        AppEvent::OpenOverlay(Overlay::ApiKeyEditor)
    ));
    assert!(matches!(
        map_key_to_event(key(KeyCode::Char('R')), &app),
        AppEvent::RefreshDeepSeekModels
    ));
}

#[tokio::test]
async fn openai_model_picker_edit_shortcut_starts_wizard_for_selected_profile() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
    app.open_overlay(Overlay::ModelPicker);
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
    .expect("app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.config.set_api_key("sk-openrouter");
    app.open_overlay(Overlay::ModelPicker);

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
    .expect("app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ModelPicker);

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
    .expect("app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.overlay = Some(Overlay::OpenAiEndpointKindPicker);
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
    .expect("app");
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
    app.open_overlay(Overlay::OpenAiProfilePicker);
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
    assert!(matches!(app.overlay, Some(Overlay::ModelPicker)));
}

#[tokio::test]
async fn saving_openai_profile_label_creates_new_openrouter_profile() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::OpenAiProfilePicker);
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
    .expect("app");
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
        app.notice
            .as_deref()
            .is_some_and(|value| value.contains("Cleared API key"))
    );
}

#[test]
fn codex_auth_detection_uses_saved_auth_storage() {
    let temp = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
    .expect("app");
    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    app.provider_picker_idx = 0;

    assert_eq!(app.selected_provider_family(), ProviderFamily::Codex);

    open_provider_family_overlay(&mut app, &oauth_manager)
        .await
        .expect("open overlay");
    assert_eq!(app.config.provider, "codex");
    assert!(matches!(app.overlay, Some(Overlay::ListPicker(_))));
}

#[tokio::test]
async fn codex_provider_family_routes_to_model_picker_with_saved_login() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");
    app.provider_picker_idx = 0;

    open_provider_family_overlay(&mut app, &oauth_manager)
        .await
        .expect("open overlay");
    assert!(matches!(app.overlay, Some(Overlay::ListPicker(_))));
    assert!(!app.codex_model_options.is_empty());
}

#[tokio::test]
async fn codex_provider_family_uses_saved_codex_provider_state() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    let oauth_manager = crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
        .expect("oauth manager");

    app.config.set_provider("ollama");
    app.config.set_api_key("sk-ollama");
    app.config.set_provider("codex");
    app.config.set_api_key("sk-codex");
    app.config.set_provider("ollama");
    app.provider_picker_idx = 0;

    assert!(codex_auth_is_available(&app, &oauth_manager));

    open_provider_family_overlay(&mut app, &oauth_manager)
        .await
        .expect("open overlay");
    assert!(matches!(app.overlay, Some(Overlay::ListPicker(_))));
}

#[tokio::test]
async fn codex_model_picker_opens_reasoning_level_overlay_before_rebuild() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    let oauth_manager = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(temp.path().join(".rara"))
            .expect("oauth manager"),
    );
    oauth_manager
        .save_api_key("sk-test-codex")
        .expect("save api key");

    app.provider_picker_idx = 0;
    open_provider_family_overlay(&mut app, &oauth_manager)
        .await
        .expect("open overlay");
    app.overlay = Some(Overlay::ModelPicker);

    let mut agent_slot = None;
    dispatch_event(
        AppEvent::ApplyOverlaySelection,
        &mut app,
        &mut agent_slot,
        &oauth_manager,
    )
    .await
    .expect("apply model selection");

    assert!(matches!(app.overlay, Some(Overlay::ReasoningEffortPicker)));
}

#[tokio::test]
async fn codex_model_picker_applies_single_reasoning_level_without_overlay() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
    app.overlay = Some(Overlay::ModelPicker);

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
        app.running_task.as_ref(),
        Some(task) if matches!(task.kind, TaskKind::Rebuild)
    ));
    if let Some(task) = app.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn codex_auth_store_is_synced_into_config_before_model_flow() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
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
    .expect("app");
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
    .expect("app");
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
