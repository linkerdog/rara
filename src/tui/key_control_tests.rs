use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};

use super::event_stream::translate_event;
use super::runtime_port::RuntimeCommand;
use super::state::{HelpTab, Overlay, QuitShortcutKey, RunningTask, TaskKind};
use super::terminal_ui::handle_paste;
use super::testing::TuiHarness;
use crate::runtime_control::SessionControlRequest;

fn ctrl(key: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL)
}

#[tokio::test]
async fn ctrl_c_closes_overlay_without_editing_draft_or_cancelling() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    tui.app_mut().bottom_pane.input = "draft".into();
    tui.app_mut().open_overlay(Overlay::Help(HelpTab::General));
    assert!(!tui.press_key(ctrl('c')).await.expect("close overlay"));
    assert!(tui.app().overlay.is_none());
    assert_eq!(tui.app().bottom_pane.input, "draft");
    tui.expect_no_commands();
}

#[tokio::test]
async fn repeated_quit_shortcut_exits_and_first_press_renders_hint() {
    for key in ['c', 'd'] {
        let mut tui = TuiHarness::new(Default::default()).expect("harness");
        assert!(!tui.press_key(ctrl(key)).await.expect("arm quit"));
        assert!(tui.screen_text(100, 30).contains(&format!(
            "Press Ctrl-{} again to quit",
            key.to_ascii_uppercase()
        )));
        assert!(tui.press_key(ctrl(key)).await.expect("confirm quit"));
        tui.expect_no_commands();
    }
}

#[tokio::test]
async fn ctrl_z_never_inserts_text() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    tui.app_mut().bottom_pane.input = "draft".into();
    assert!(!tui.press_key(ctrl('z')).await.expect("suspend intent"));
    assert_eq!(tui.app().bottom_pane.input, "draft");
    tui.expect_no_commands();
}

#[tokio::test]
async fn busy_ctrl_c_interrupts_once_then_quits_even_before_task_returns() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    tui.app_mut().bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(std::future::pending()),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: None,
    });
    tui.app_mut().bottom_pane.input = "follow-up draft".into();
    tui.app_mut().open_overlay(Overlay::Help(HelpTab::General));
    assert!(!tui.press_key(ctrl('c')).await.expect("dismiss only"));
    tui.expect_no_commands();
    assert_eq!(tui.app().quit_shortcut.key(), None);
    assert!(!tui.press_key(ctrl('c')).await.expect("interrupt and arm"));
    assert_eq!(tui.app().bottom_pane.input, "follow-up draft");
    assert!(tui.app().is_busy());
    assert!(
        tui.press_key(ctrl('c'))
            .await
            .expect("confirm while draining")
    );
    tui.expect_command(RuntimeCommand::Session(
        SessionControlRequest::CancelCurrentTurn,
    ));
    let task = tui.app_mut().bottom_pane.running_task.take().expect("task");
    task.handle.abort();
    assert!(matches!(task.handle.await, Err(error) if error.is_cancelled()));
}

#[tokio::test]
async fn palette_ctrl_c_discards_pending_paste_before_it_can_hide_the_palette() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    handle_paste("/model".into(), tui.app_mut());
    assert_eq!(tui.app().overlay, Some(Overlay::CommandPalette));
    handle_paste(" argument\nsecond".into(), tui.app_mut());
    assert!(!tui.press_key(ctrl('c')).await.expect("dismiss palette"));
    assert!(tui.app().overlay.is_none());
    assert!(tui.app().bottom_pane.input.is_empty());
    assert!(!tui.app_mut().flush_composer_paste());
    assert_eq!(tui.app().quit_shortcut.key(), None);
    tui.expect_no_commands();
}

#[tokio::test]
async fn ctrl_d_edits_owned_fields_without_arming_quit_or_inserting_a_letter() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    handle_paste("draft".into(), tui.app_mut());
    tui.app_mut().bottom_pane.input_cursor_offset = Some(0);
    assert!(!tui.press_key(ctrl('d')).await.expect("delete forward"));
    assert_eq!(tui.app().bottom_pane.input, "raft");
    assert_eq!(tui.app().quit_shortcut.key(), None);
    tui.app_mut().open_overlay(Overlay::ModelSearch);
    handle_paste("model".into(), tui.app_mut());
    tui.press_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE))
        .await
        .expect("query home");
    assert!(!tui.press_key(ctrl('d')).await.expect("query delete"));
    assert_eq!(tui.app().model_search_query, "odel");
    assert_eq!(tui.app().bottom_pane.input, "raft");
    tui.app_mut().open_overlay(Overlay::Help(HelpTab::General));
    assert!(!tui.press_key(ctrl('d')).await.expect("read-only input"));
    assert_eq!(tui.app().overlay, Some(Overlay::Help(HelpTab::General)));
    assert_eq!(tui.app().quit_shortcut.key(), None);
    tui.expect_no_commands();
}

#[tokio::test]
async fn pending_paste_prevents_empty_composer_ctrl_d_quit() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    assert!(!tui.press_key(ctrl('d')).await.expect("arm"));
    handle_paste("line one\nline two".into(), tui.app_mut());
    assert!(!tui.press_key(ctrl('d')).await.expect("flush and edit"));
    assert_eq!(tui.app().quit_shortcut.key(), None);
    assert_eq!(tui.app().bottom_pane.input, "line one\nline two");
}

#[tokio::test]
async fn different_shortcuts_and_ordinary_input_do_not_confirm_quit() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    for key in ['c', 'd', 'c'] {
        assert!(!tui.press_key(ctrl(key)).await.expect("rearm"));
    }
    assert!(
        !tui.press_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .await
            .expect("type")
    );
    assert_eq!(tui.app().quit_shortcut.key(), None);
    assert!(!tui.press_key(ctrl('c')).await.expect("clear and rearm"));
    assert!(tui.app().bottom_pane.input.is_empty());
    assert!(tui.press_key(ctrl('c')).await.expect("confirm"));
}

#[tokio::test]
async fn passive_pointer_motion_preserves_quit_confirmation() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    assert!(!tui.press_key(ctrl('c')).await.expect("arm"));
    tui.send_terminal_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 3,
        row: 2,
        modifiers: KeyModifiers::NONE,
    }))
    .await
    .expect("move pointer");
    assert_eq!(tui.app().quit_shortcut.key(), Some(QuitShortcutKey::CtrlC));
    assert!(tui.press_key(ctrl('c')).await.expect("confirm"));
}

#[tokio::test]
async fn paste_mouse_and_suspend_disarm_confirmation() {
    for event in [
        Event::Paste("text".into()),
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }),
        Event::Key(ctrl('z')),
    ] {
        let mut tui = TuiHarness::new(Default::default()).expect("harness");
        assert!(!tui.press_key(ctrl('c')).await.expect("arm"));
        translate_event(event, tui.app_mut());
        assert_eq!(tui.app().quit_shortcut.key(), None);
        assert!(!tui.press_key(ctrl('c')).await.expect("rearm after input"));
    }
}

#[tokio::test]
async fn reported_repeats_and_releases_cannot_confirm_or_suspend() {
    for key in ['c', 'd', 'z'] {
        let mut tui = TuiHarness::new(Default::default()).expect("harness");
        assert!(!tui.press_key(ctrl('c')).await.expect("arm"));
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            assert!(
                translate_event(
                    Event::Key(KeyEvent::new_with_kind(
                        KeyCode::Char(key),
                        KeyModifiers::CONTROL,
                        kind
                    )),
                    tui.app_mut()
                )
                .is_none()
            );
        }
        assert_eq!(tui.app().quit_shortcut.key(), Some(QuitShortcutKey::CtrlC));
    }
}

#[test]
fn expired_confirmation_restores_footer_without_another_key() {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    let now = Instant::now();
    tui.app_mut()
        .quit_shortcut
        .press(QuitShortcutKey::CtrlD, now);
    assert!(
        tui.screen_text(100, 30)
            .contains("Press Ctrl-D again to quit")
    );
    assert!(
        tui.app_mut()
            .quit_shortcut
            .expire(now + Duration::from_secs(1))
    );
    let screen = tui.screen_text(100, 30);
    assert!(!screen.contains("again to quit"));
    assert!(screen.contains("perm="));
}
