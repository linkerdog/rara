use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, style::Modifier};

use crate::tui::selection::ScreenPosition;
use crate::tui::state::{RuntimePhase, RuntimeSnapshot, TranscriptEntry, TranscriptTurn};
use crate::tui::testing::TuiHarness;

fn numbered_rows(range: std::ops::Range<usize>) -> String {
    range
        .map(|row| format!("ROW-{row:05}\n"))
        .collect::<String>()
}

fn harness_with_rows(rows: usize) -> TuiHarness {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    harness
        .app_mut()
        .restore_committed_turns(vec![TranscriptTurn {
            thinking_duration: None,
            entries: vec![TranscriptEntry::new(
                "Agent",
                format!("```text\n{}```", numbered_rows(0..rows)),
            )],
        }]);
    harness
}

fn streaming_harness() -> TuiHarness {
    let mut harness = harness_with_rows(10);
    let app = harness.app_mut();
    app.push_entry("You", "stream a numbered response");
    app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    app.append_agent_delta(&format!("```text\n{}", numbered_rows(10..70)));
    harness
}

async fn press(harness: &mut TuiHarness, code: KeyCode) {
    assert!(
        !harness
            .press_key(KeyEvent::new(code, KeyModifiers::NONE))
            .await
            .expect("dispatch scroll key")
    );
    harness.expect_no_commands();
}

fn buffer_row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[tokio::test]
async fn overscrolling_the_top_does_not_delay_the_next_page_down() {
    let mut harness = harness_with_rows(80);
    harness.screen_buffer(80, 20);
    for _ in 0..40 {
        press(&mut harness, KeyCode::PageUp).await;
    }
    let top = harness.screen_text(80, 20);
    assert!(top.contains("ROW-00000"));

    press(&mut harness, KeyCode::PageDown).await;
    let moved = harness.screen_text(80, 20);
    assert!(
        !moved.contains("ROW-00000"),
        "one PageDown must move immediately after repeated PageUp at the top"
    );
}

#[tokio::test]
async fn streamed_appends_preserve_the_manual_top_visual_row() {
    let mut harness = streaming_harness();
    harness.screen_buffer(80, 20);
    press(&mut harness, KeyCode::PageUp).await;
    let (before, _) = harness.screen_buffer(80, 20);
    let visible = (0..5).map(|y| buffer_row(&before, y)).collect::<Vec<_>>();
    assert!(visible.iter().any(|row| row.contains("ROW-")));

    harness.app_mut().append_agent_delta(&numbered_rows(70..90));
    let (after, _) = harness.screen_buffer(80, 20);
    assert_eq!(
        (0..5).map(|y| buffer_row(&after, y)).collect::<Vec<_>>(),
        visible,
        "appended rows must not move an up-scrolled viewport"
    );
}

#[test]
fn seventy_thousand_row_tail_is_rendered_highlighted_and_copied() {
    let mut harness = harness_with_rows(70_000);
    let (buffer, _) = harness.screen_buffer(80, 20);
    assert!(
        harness
            .app()
            .transcript_scroll
            .layout()
            .expect("measured viewport")
            .content_rows
            >= 70_000
    );
    assert!(harness.app().transcript_scroll.offset() > usize::from(u16::MAX));
    let tail = "ROW-69999";
    let (x, y) = (0..buffer.area.height)
        .find_map(|y| buffer_row(&buffer, y).find(tail).map(|x| (x as u16, y)))
        .expect("the actual tail beyond 65,535 visual rows must be rendered");
    let selection = &mut harness.app_mut().transcript_selection;
    assert!(selection.start(ScreenPosition::new(x, y)));
    assert!(selection.drag(ScreenPosition::new(x + tail.len() as u16, y)));
    assert_eq!(selection.selected_text().as_deref(), Some(tail));
    let (highlighted, _) = harness.screen_buffer(80, 20);
    let copied_cells = (x..x + tail.len() as u16)
        .map(|x| {
            let cell = &highlighted[(x, y)];
            assert!(cell.modifier.contains(Modifier::REVERSED));
            cell.symbol()
        })
        .collect::<String>();
    assert_eq!(copied_cells, tail);

    harness.app_mut().transcript_selection.clear();
    super::scroll_transcript(harness.app_mut(), -4_000);
    assert!(harness.app().transcript_scroll.offset() > usize::from(u16::MAX));
    let (middle, _) = harness.screen_buffer(80, 20);
    let first_row = buffer_row(&middle, 0);
    let row_number: usize = first_row
        .trim()
        .strip_prefix("ROW-")
        .expect("numbered middle row")
        .parse()
        .expect("row number");
    assert!(row_number > usize::from(u16::MAX) && row_number < 69_000);
    super::scroll_transcript(harness.app_mut(), i32::MIN);
    assert!(harness.screen_text(80, 20).contains("ROW-00000"));
    super::scroll_transcript(harness.app_mut(), i32::MAX);
    assert!(harness.screen_text(80, 20).contains(tail));
}

#[tokio::test]
async fn scroll_input_refreshes_stream_bounds_before_the_next_frame() {
    let mut deferred = streaming_harness();
    let mut rendered = streaming_harness();
    deferred.screen_buffer(80, 20);
    rendered.screen_buffer(80, 20);
    deferred
        .app_mut()
        .append_agent_delta(&numbered_rows(70..90));
    rendered
        .app_mut()
        .append_agent_delta(&numbered_rows(70..90));
    rendered.screen_buffer(80, 20);

    press(&mut deferred, KeyCode::PageUp).await;
    press(&mut rendered, KeyCode::PageUp).await;
    let (deferred_buffer, _) = deferred.screen_buffer(80, 20);
    let (rendered_buffer, _) = rendered.screen_buffer(80, 20);
    assert_eq!(
        (0..5)
            .map(|y| buffer_row(&deferred_buffer, y))
            .collect::<Vec<_>>(),
        (0..5)
            .map(|y| buffer_row(&rendered_buffer, y))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn scroll_before_the_first_frame_does_not_break_tail_following() {
    let mut harness = harness_with_rows(80);
    for _ in 0..40 {
        press(&mut harness, KeyCode::PageUp).await;
    }
    assert!(harness.screen_text(80, 20).contains("ROW-00079"));
}

#[tokio::test]
async fn edge_autoscroll_uses_the_shared_clamped_scroll_path() {
    let mut harness = harness_with_rows(80);
    harness.screen_buffer(80, 20);
    press(&mut harness, KeyCode::PageUp).await;
    let (before, _) = harness.screen_buffer(80, 20);
    let app = harness.app_mut();
    let initial_offset = app.transcript_scroll.offset();
    let height = app
        .transcript_scroll
        .layout()
        .expect("measured viewport")
        .height;
    assert!(app.transcript_selection.start(ScreenPosition::new(4, 1)));
    assert!(
        app.transcript_selection
            .drag(ScreenPosition::new(8, height))
    );
    let delta = app
        .transcript_selection
        .autoscroll_delta()
        .expect("edge scroll");
    assert_eq!(delta, 1);
    super::scroll_transcript(app, delta);
    assert_eq!(app.transcript_scroll.offset(), initial_offset + 1);
    let (after, _) = harness.screen_buffer(80, 20);
    assert_eq!(buffer_row(&after, 0), buffer_row(&before, 1));
    assert!(harness.app().transcript_selection.selected_text().is_some());
}

#[tokio::test]
async fn clear_and_thread_restore_explicitly_resume_tail_following() {
    let mut harness = harness_with_rows(80);
    let turns = harness.app().committed_turns.clone();
    harness.screen_buffer(80, 20);
    press(&mut harness, KeyCode::PageUp).await;
    harness.app_mut().reset_transcript();
    assert_eq!(harness.app().transcript_scroll.offset(), 0);
    assert!(harness.app().transcript_scroll.layout().is_none());
    harness.app_mut().restore_committed_turns(turns.clone());
    assert!(harness.screen_text(80, 20).contains("ROW-00079"));
    press(&mut harness, KeyCode::PageUp).await;
    harness.app_mut().restore_committed_turns(turns);
    assert!(harness.screen_text(80, 20).contains("ROW-00079"));
}
