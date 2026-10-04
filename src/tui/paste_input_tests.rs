use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::runtime_port::RuntimeCommand;
use super::state::{
    InteractionKind, Overlay, PendingApprovalSnapshot, PendingInteractionSnapshot, RunningTask,
    RuntimeSnapshot, TaskKind,
};
use super::terminal_ui::handle_paste;
use super::testing::TuiHarness;
use crate::oauth::OAuthManager;
use crate::runtime_control::{InputControlRequest, SessionControlRequest};

fn harness() -> TuiHarness {
    TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness")
}

async fn press(tui: &mut TuiHarness, code: KeyCode, modifiers: KeyModifiers) {
    assert!(
        !tui.press_key(KeyEvent::new(code, modifiers))
            .await
            .expect("dispatch key")
    );
}

#[tokio::test]
async fn submit_immediately_after_paste_includes_full_text_and_clears_pending_state() {
    for paste in ["first\nsecond".to_string(), "x".repeat(1200)] {
        let mut tui = harness();
        handle_paste(paste.clone(), tui.app_mut());
        press(&mut tui, KeyCode::Enter, KeyModifiers::NONE).await;
        tui.expect_command(RuntimeCommand::Input(
            InputControlRequest::SubmitUserPrompt { prompt: paste },
        ));
        assert!(tui.app().bottom_pane.input.is_empty());
        assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
        assert!(!tui.app_mut().bottom_pane.flush_paste_burst());
        assert!(
            tui.app().bottom_pane.notice.is_none(),
            "submitted paste notice"
        );
    }
}

#[tokio::test]
async fn submission_retires_paste_notices_but_preserves_later_warnings() {
    for paste in [
        "first\nsecond".to_string(),
        "x".repeat(1200),
        " \n \n".into(),
    ] {
        for warning in [
            None,
            Some("Network unavailable"),
            Some("Pasted content is prohibited by policy"),
        ] {
            let mut tui = harness();
            handle_paste(paste.clone(), tui.app_mut());
            assert!(tui.app_mut().flush_composer_paste());
            if let Some(warning) = warning {
                tui.app_mut().bottom_pane.notice = Some(warning.into());
            }
            press(&mut tui, KeyCode::Enter, KeyModifiers::NONE).await;
            let whitespace_only = paste.trim().is_empty();
            assert_eq!(
                tui.app().bottom_pane.notice.as_deref(),
                warning.or(if whitespace_only {
                    Some("Ready.")
                } else {
                    None
                })
            );
            assert!(tui.app().bottom_pane.input.is_empty());
            assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
            assert!(!tui.app_mut().flush_composer_paste());
            if whitespace_only {
                tui.expect_no_commands();
            } else {
                tui.expect_command(RuntimeCommand::Input(
                    InputControlRequest::SubmitUserPrompt {
                        prompt: paste.clone(),
                    },
                ));
            }
        }
    }
}

#[tokio::test]
async fn approval_shortcut_edits_the_flushed_pasted_draft() {
    let mut tui = harness();
    tui.app_mut()
        .snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::Approval,
            title: "Approve command".into(),
            summary: "cargo check".into(),
            options: Vec::new(),
            note: None,
            approval: Some(PendingApprovalSnapshot {
                tool_use_id: "approval-1".into(),
                command: "cargo check".into(),
                allow_net: false,
                payload: crate::tools::bash::BashCommandInput::from_value(
                    serde_json::json!({"command": "cargo check"}),
                )
                .expect("approval payload"),
            }),
            source: None,
            created_at_epoch_seconds: None,
        });
    handle_paste("first\nsecond".into(), tui.app_mut());
    assert!(
        matches!(
            super::keymap::map_key_to_event(
                KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
                tui.app()
            ),
            super::app_event::AppEvent::SelectPendingOption(0)
        ),
        "the unflushed draft would route to the approval shortcut"
    );
    press(&mut tui, KeyCode::Char('1'), KeyModifiers::NONE).await;
    assert_eq!(tui.app().bottom_pane.input, "first\nsecond1");
    assert!(tui.app().active_pending_interaction().is_some());
    tui.expect_no_commands();
}

#[tokio::test]
async fn direct_submit_expands_pending_paste_before_consuming_the_draft() {
    let dir = tempfile::tempdir().expect("tempdir");
    let oauth =
        Arc::new(OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth"));
    let mut tui = harness();
    let runtime = super::testing::FakeRuntimeClient::new(tui.app().snapshot.clone().into_inner());
    let paste = "x".repeat(1200);
    handle_paste(paste.clone(), tui.app_mut());
    super::submit::handle_submit_with_port(tui.app_mut(), &mut None, &oauth, &runtime)
        .await
        .expect("direct submit");
    assert_eq!(
        runtime.commands(),
        vec![RuntimeCommand::Input(
            InputControlRequest::SubmitUserPrompt { prompt: paste }
        )]
    );
    assert!(tui.app().bottom_pane.input.is_empty());
    assert!(tui.app().bottom_pane.notice.is_none());
    assert!(!tui.app_mut().flush_composer_paste());
}

#[tokio::test]
async fn paste_is_inserted_at_original_cursor_before_next_edit() {
    let mut tui = harness();
    tui.app_mut().bottom_pane.input = "before after".into();
    tui.app_mut().bottom_pane.input_cursor_offset = Some(7);
    handle_paste("first\nsecond".into(), tui.app_mut());
    press(&mut tui, KeyCode::Char('!'), KeyModifiers::NONE).await;
    assert_eq!(tui.app().bottom_pane.input, "before first\nsecond!after");
    assert!(!tui.app_mut().bottom_pane.flush_paste_burst());
    tui.expect_no_commands();
}

#[tokio::test]
async fn clear_removes_pending_paste_and_flushed_placeholder_payloads() {
    for (paste, flush_first) in [
        ("first\nsecond".to_string(), false),
        ("first\nsecond".to_string(), true),
        ("x".repeat(1200), false),
        ("x".repeat(1200), true),
    ] {
        let mut tui = harness();
        handle_paste(paste, tui.app_mut());
        if flush_first {
            tui.app_mut().bottom_pane.flush_paste_burst();
        }
        press(&mut tui, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        assert!(tui.app().bottom_pane.input.is_empty());
        assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
        assert_eq!(tui.app().bottom_pane.large_paste_counter, 0);
        assert!(!tui.app_mut().bottom_pane.flush_paste_burst());
        assert!(
            tui.app().bottom_pane.notice.is_none(),
            "discarded paste notice"
        );
        tui.expect_no_commands();
    }
}

#[tokio::test]
async fn clear_preserves_unrelated_notices_after_flushing_a_paste() {
    for warning in [
        "Network unavailable",
        "Pasted content is prohibited by policy",
    ] {
        let mut tui = harness();
        handle_paste("x".repeat(1200), tui.app_mut());
        assert!(tui.app_mut().flush_composer_paste());
        tui.app_mut().bottom_pane.notice = Some(warning.into());
        press(&mut tui, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        assert!(tui.app().bottom_pane.input.is_empty());
        assert_eq!(tui.app().bottom_pane.notice.as_deref(), Some(warning));
        tui.expect_no_commands();
    }
}

#[test]
fn palette_dismissal_discards_pending_burst_and_large_payloads() {
    for flush_first in [false, true] {
        let mut tui = harness();
        tui.app_mut().bottom_pane.input = "/".into();
        tui.app_mut().open_overlay(Overlay::CommandPalette);
        tui.app_mut()
            .bottom_pane
            .handle_paste_burst_chunk(&"x".repeat(1200));
        if flush_first {
            tui.app_mut().bottom_pane.flush_paste_burst();
        }
        tui.app_mut().dismiss_overlay();
        assert!(tui.app().bottom_pane.input.is_empty());
        assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
        assert!(
            !tui.app_mut().bottom_pane.flush_paste_burst(),
            "dismissed burst must not reappear"
        );
        assert!(tui.app().bottom_pane.notice.is_none());
        tui.expect_no_commands();
    }
}

#[tokio::test]
async fn palette_escape_discards_paste_before_key_routing() {
    for paste in ["first\nsecond".to_string(), "x".repeat(1200)] {
        for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
            let mut tui = harness();
            tui.app_mut().bottom_pane.input = "/".into();
            tui.app_mut().open_overlay(Overlay::CommandPalette);
            handle_paste(paste.clone(), tui.app_mut());
            assert!(
                !tui.press_key(KeyEvent::new_with_kind(
                    KeyCode::Esc,
                    KeyModifiers::NONE,
                    kind
                ))
                .await
                .expect("palette Esc")
            );
            assert!(tui.app().overlay.is_none());
            assert!(tui.app().bottom_pane.input.is_empty());
            assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
            assert!(tui.app().bottom_pane.notice.is_none());
            assert!(!tui.app_mut().flush_composer_paste());
            tui.expect_no_commands();
        }
    }
}

#[tokio::test]
async fn direct_palette_close_discards_paste_before_action_flush() {
    let dir = tempfile::tempdir().expect("tempdir");
    let oauth =
        Arc::new(OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth"));
    for paste in ["first\nsecond".to_string(), "x".repeat(1200)] {
        let mut tui = harness();
        tui.app_mut().bottom_pane.input = "/".into();
        tui.app_mut().open_overlay(Overlay::CommandPalette);
        handle_paste(paste, tui.app_mut());
        super::event_dispatch::dispatch_event(
            super::app_event::AppEvent::CloseOverlay,
            tui.app_mut(),
            &mut None,
            &oauth,
        )
        .await
        .expect("direct close");
        assert!(tui.app().overlay.is_none());
        assert!(tui.app().bottom_pane.input.is_empty());
        assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
        assert!(tui.app().bottom_pane.notice.is_none());
        assert!(!tui.app_mut().flush_composer_paste());
    }
}

#[tokio::test]
async fn idle_escape_preserves_pasted_draft_without_a_later_flush() {
    let mut tui = harness();
    handle_paste("first\nsecond".into(), tui.app_mut());
    press(&mut tui, KeyCode::Esc, KeyModifiers::NONE).await;
    assert_eq!(tui.app().bottom_pane.input, "first\nsecond");
    assert!(!tui.app_mut().bottom_pane.flush_paste_burst());
    tui.expect_no_commands();
}

#[tokio::test]
async fn history_key_routing_uses_the_pasted_multiline_composer() {
    let mut tui = harness();
    tui.app_mut().record_input_history("old prompt");
    handle_paste("first\nsecond".into(), tui.app_mut());
    press(&mut tui, KeyCode::Up, KeyModifiers::NONE).await;
    assert_eq!(tui.app().bottom_pane.input, "first\nsecond");
    assert!(tui.app().composer_cursor_offset() < "first\nsecond".len());
    assert!(!tui.app_mut().bottom_pane.flush_paste_burst());
    tui.expect_no_commands();
}

#[tokio::test]
async fn small_paste_after_pending_multiline_paste_keeps_event_order() {
    let mut tui = harness();
    handle_paste("first\nsecond".into(), tui.app_mut());
    handle_paste(" tail".into(), tui.app_mut());
    press(&mut tui, KeyCode::Enter, KeyModifiers::NONE).await;
    tui.expect_command(RuntimeCommand::Input(
        InputControlRequest::SubmitUserPrompt {
            prompt: "first\nsecond tail".into(),
        },
    ));
    assert!(!tui.app_mut().bottom_pane.flush_paste_burst());
}

#[tokio::test]
async fn escape_during_a_turn_requests_cancellation_and_preserves_pending_draft() {
    let mut tui = harness();
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    tui.app_mut().bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(std::future::pending()),
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: None,
    });
    handle_paste("first\nsecond".into(), tui.app_mut());
    press(&mut tui, KeyCode::Esc, KeyModifiers::NONE).await;
    tui.expect_command(RuntimeCommand::Session(
        SessionControlRequest::CancelCurrentTurn,
    ));
    assert_eq!(tui.app().bottom_pane.input, "first\nsecond");
    assert!(!tui.app_mut().bottom_pane.flush_paste_burst());
    let task = tui
        .app_mut()
        .bottom_pane
        .running_task
        .take()
        .expect("scripted task");
    task.handle.abort();
    assert!(matches!(task.handle.await, Err(error) if error.is_cancelled()));
}

#[tokio::test]
async fn released_keys_do_not_flush_paste_or_submit() {
    let mut tui = harness();
    handle_paste("first\nsecond".into(), tui.app_mut());
    assert!(
        !tui.press_key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ))
        .await
        .expect("dispatch released key")
    );
    assert!(tui.app().bottom_pane.input.is_empty());
    tui.expect_no_commands();
    press(&mut tui, KeyCode::Enter, KeyModifiers::NONE).await;
    tui.expect_command(RuntimeCommand::Input(
        InputControlRequest::SubmitUserPrompt {
            prompt: "first\nsecond".into(),
        },
    ));
}
