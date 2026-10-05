use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::app_event::AppEvent;
use super::event_stream::{UiEvent, translate_event};
use super::message_role::MessageRole;
use super::state::{Overlay, TranscriptEntry, TranscriptTurn};
use super::testing::TuiHarness;

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

async fn dragging_harness() -> TuiHarness {
    let mut harness = TuiHarness::new(Default::default()).expect("isolated harness");
    let rows = (0..80)
        .map(|row| format!("ROW-{row:05}\n"))
        .collect::<String>();
    harness
        .app_mut()
        .restore_committed_turns(vec![TranscriptTurn {
            thinking_duration: None,
            entries: vec![TranscriptEntry::new(
                MessageRole::Agent,
                format!("```text\n{rows}```"),
            )],
        }]);
    harness.screen_buffer(80, 20);
    harness
        .press_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .await
        .expect("scroll away from tail");
    harness.screen_buffer(80, 20);
    let height = harness
        .app()
        .transcript_scroll
        .layout()
        .expect("measured transcript")
        .height;
    for event in [
        mouse(MouseEventKind::Down(MouseButton::Left), 4, 1),
        mouse(MouseEventKind::Drag(MouseButton::Left), 8, height),
    ] {
        harness
            .send_terminal_event(event)
            .await
            .expect("start drag through terminal dispatch");
    }
    assert!(harness.app().transcript_selection.is_dragging());
    assert!(harness.app().transcript_selection.selected_text().is_some());
    assert_eq!(
        harness.app_mut().transcript_selection.autoscroll_delta(),
        Some(1)
    );
    harness.expect_no_commands();
    harness
}

fn assert_cancelled(harness: &mut TuiHarness) {
    assert!(!harness.app().transcript_selection.is_dragging());
    assert!(harness.app().transcript_selection.selected_text().is_none());
    assert_eq!(
        harness.app_mut().transcript_selection.autoscroll_delta(),
        None
    );
    assert!(
        harness.app().clipboard.is_none(),
        "cancellation must not copy"
    );
}

async fn assert_orphaned_events_are_ignored(harness: &mut TuiHarness) {
    for kind in [
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        let event = mouse(kind, 8, 1);
        assert!(matches!(
            translate_event(event.clone(), harness.app_mut()),
            Some(UiEvent::App(AppEvent::Noop))
        ));
        harness
            .send_terminal_event(event)
            .await
            .expect("late event");
        assert_cancelled(harness);
    }
}

async fn assert_wheel_scrolls(harness: &mut TuiHarness) {
    let before = harness.app().transcript_scroll.offset();
    harness
        .send_terminal_event(mouse(MouseEventKind::ScrollDown, 8, 1))
        .await
        .expect("wheel dispatch");
    assert!(harness.app().transcript_scroll.offset() > before);
    assert_cancelled(harness);
}

#[tokio::test]
async fn opening_an_overlay_cancels_drag_before_render_and_routes_the_wheel() {
    let mut harness = dragging_harness().await;
    harness.app_mut().open_overlay(Overlay::CommandPalette);
    assert_cancelled(&mut harness);
    let before = harness.app().transcript_scroll.offset();
    harness
        .send_terminal_event(mouse(MouseEventKind::ScrollDown, 8, 1))
        .await
        .expect("palette wheel");
    assert!(harness.app().command_palette_idx > 0);
    assert_eq!(harness.app().transcript_scroll.offset(), before);
    assert_orphaned_events_are_ignored(&mut harness).await;
    harness
        .press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .await
        .expect("dismiss palette");
    assert!(harness.app().overlay.is_none());
    assert_orphaned_events_are_ignored(&mut harness).await;
    assert_wheel_scrolls(&mut harness).await;
}

#[tokio::test]
async fn focus_loss_cancels_drag_and_focus_gain_does_not_resume_it() {
    let mut harness = dragging_harness().await;
    harness
        .send_terminal_event(Event::FocusLost)
        .await
        .expect("blur");
    assert_cancelled(&mut harness);
    harness
        .send_terminal_event(Event::FocusGained)
        .await
        .expect("focus");
    assert_orphaned_events_are_ignored(&mut harness).await;
    assert_wheel_scrolls(&mut harness).await;
}

#[tokio::test]
async fn no_button_motion_recovers_a_lost_release_and_requests_a_redraw() {
    let mut harness = dragging_harness().await;
    let motion = mouse(MouseEventKind::Moved, 8, 1);
    assert!(matches!(
        translate_event(motion.clone(), harness.app_mut()),
        Some(UiEvent::Draw)
    ));
    assert_cancelled(&mut harness);
    assert!(translate_event(motion, harness.app_mut()).is_none());
    assert_orphaned_events_are_ignored(&mut harness).await;
    assert_wheel_scrolls(&mut harness).await;
}

#[tokio::test]
async fn wheel_recovers_a_lost_release_and_scrolls_on_the_same_event() {
    for kind in [MouseEventKind::ScrollUp, MouseEventKind::ScrollDown] {
        let mut harness = dragging_harness().await;
        let before = harness.app().transcript_scroll.offset();
        harness
            .send_terminal_event(mouse(kind, 8, 1))
            .await
            .expect("wheel");
        let after = harness.app().transcript_scroll.offset();
        match kind {
            MouseEventKind::ScrollUp => assert!(after < before),
            MouseEventKind::ScrollDown => assert!(after > before),
            _ => unreachable!("fixture contains only vertical wheel events"),
        }
        assert_cancelled(&mut harness);
        assert_orphaned_events_are_ignored(&mut harness).await;
    }
}

#[tokio::test]
async fn a_new_press_outside_the_transcript_cancels_the_old_drag() {
    let mut harness = dragging_harness().await;
    harness
        .send_terminal_event(mouse(MouseEventKind::Down(MouseButton::Left), 4, 19))
        .await
        .expect("click outside transcript");
    assert_cancelled(&mut harness);
    assert_orphaned_events_are_ignored(&mut harness).await;
    assert_wheel_scrolls(&mut harness).await;
}

#[tokio::test]
async fn drag_and_release_without_a_press_cannot_start_selection() {
    let mut harness = TuiHarness::new(Default::default()).expect("isolated harness");
    assert_orphaned_events_are_ignored(&mut harness).await;
}

#[cfg(unix)]
#[tokio::test]
async fn suspension_cancels_drag_before_releasing_terminal_ownership() {
    let mut harness = dragging_harness().await;
    assert!(matches!(
        translate_event(
            Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL)),
            harness.app_mut(),
        ),
        Some(UiEvent::Suspend)
    ));
    assert_cancelled(&mut harness);
    assert_orphaned_events_are_ignored(&mut harness).await;
    assert_wheel_scrolls(&mut harness).await;
}
