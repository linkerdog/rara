use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::runtime_port::RuntimeCommand;
use super::state::{RunningTask, RuntimeSnapshot, TaskKind};
use super::terminal_ui::handle_paste;
use super::testing::TuiHarness;
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
    }
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
    for flush_first in [false, true] {
        let mut tui = harness();
        handle_paste("x".repeat(1200), tui.app_mut());
        if flush_first {
            tui.app_mut().bottom_pane.flush_paste_burst();
        }
        press(&mut tui, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        assert!(tui.app().bottom_pane.input.is_empty());
        assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
        assert_eq!(tui.app().bottom_pane.large_paste_counter, 0);
        assert!(!tui.app_mut().bottom_pane.flush_paste_burst());
        tui.expect_no_commands();
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
        cancellation_requested: false,
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
