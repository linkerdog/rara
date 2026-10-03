use super::*;

#[test]
fn first_dirty_frame_is_immediate_and_idle_has_no_deadline() {
    let now = Instant::now();
    let mut frames = FrameScheduler::default();
    assert!(!frames.is_due(now));
    assert_eq!(frames.deadline, None);
    frames.request(now);
    assert!(frames.is_due(now));
    frames.mark_drawn(now);
    assert!(!frames.is_due(now + Duration::from_secs(1)));
    assert_eq!(frames.deadline, None);
}

#[test]
fn burst_requests_produce_one_trailing_frame_not_one_per_event() {
    let start = Instant::now();
    let mut frames = FrameScheduler::default();
    frames.request(start);
    frames.mark_drawn(start);
    let mut draws = 0;
    for micros in 1..=1_000 {
        let now = start + Duration::from_micros(micros);
        frames.request(now);
        if frames.is_due(now) {
            draws += 1;
            frames.mark_drawn(now);
        }
    }
    assert_eq!(
        draws, 0,
        "a one-millisecond burst must not repaint per event"
    );
    let due = start + Duration::from_nanos(16_666_667);
    assert!(frames.is_due(due));
    frames.mark_drawn(due);
    draws += 1;
    assert_eq!(draws, 1);
    assert_eq!(frames.deadline, None);
}

#[test]
fn continuous_requests_cannot_postpone_the_pending_frame() {
    let start = Instant::now();
    let mut frames = FrameScheduler::default();
    frames.mark_drawn(start);
    frames.request(start + Duration::from_millis(1));
    let deadline = frames.deadline;
    for millis in 2..=100 {
        frames.request(start + Duration::from_millis(millis));
        assert_eq!(frames.deadline, deadline);
    }
    assert_eq!(deadline, Some(start + Duration::from_nanos(16_666_667)));
}

#[test]
fn late_paint_uses_actual_completion_time_without_a_catch_up_burst() {
    let start = Instant::now();
    let mut frames = FrameScheduler::default();
    frames.mark_drawn(start);
    frames.request(start + Duration::from_millis(1));
    let late = start + Duration::from_secs(1);
    assert!(frames.is_due(late));
    frames.mark_drawn(late);
    frames.request(late);
    assert!(!frames.is_due(late));
    assert_eq!(
        frames.deadline,
        Some(late + Duration::from_nanos(16_666_667))
    );
}

#[test]
fn continuous_traffic_draw_count_is_bounded_and_retains_the_final_request() {
    let start = Instant::now();
    let mut frames = FrameScheduler::default();
    let mut draws = 0;
    for millis in 0..1_000 {
        let now = start + Duration::from_millis(millis);
        frames.request(now);
        if frames.is_due(now) {
            draws += 1;
            frames.mark_drawn(now);
        }
    }
    assert!(draws <= 60, "one second of traffic produced {draws} frames");
    assert!(frames.deadline.is_some(), "the final request must survive");
    assert!(frames.is_due(start + Duration::from_millis(1_017)));
}

#[test]
fn a_request_after_an_idle_interval_does_not_add_extra_latency() {
    let start = Instant::now();
    let mut frames = FrameScheduler::default();
    frames.mark_drawn(start);
    let later = start + Duration::from_secs(1);
    frames.request(later);
    assert_eq!(frames.deadline, Some(later));
    assert!(frames.is_due(later));
}

#[tokio::test]
async fn idle_wait_remains_pending_without_a_polling_timer() {
    let frames = FrameScheduler::default();
    let wait = frames.wait();
    tokio::pin!(wait);
    assert!(futures::poll!(&mut wait).is_pending());
}

#[tokio::test]
async fn overdue_frame_wakes_without_another_runtime_or_input_event() {
    let mut frames = FrameScheduler::default();
    let now = Instant::now();
    frames.request(now - Duration::from_secs(1));
    frames.wait().await;
    assert!(frames.is_due(now));
    frames.mark_drawn(now);
    let wait = frames.wait();
    tokio::pin!(wait);
    assert!(futures::poll!(&mut wait).is_pending());
}

#[tokio::test]
async fn coalesced_controller_events_render_the_complete_ordered_final_projection() {
    use std::sync::Arc;

    use ratatui::{buffer::Buffer, layout::Rect};

    use crate::runtime_control::{
        AssistantEvent, RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, SessionEvent,
    };
    use crate::tui::controller::TuiController;
    use crate::tui::custom_terminal::Frame;
    use crate::tui::runtime_port::RuntimeProjectionEvent;
    use crate::tui::state::{RuntimeSnapshot, TuiApp};
    use crate::tui::testing::FakeRuntimeClient;

    let temp = tempfile::tempdir().unwrap();
    let mut app = TuiApp::new(crate::config::ConfigManager {
        path: temp.path().join("config.json"),
    })
    .unwrap();
    app.push_entry("You", "Stream the numbered tokens.");
    let port = Arc::new(FakeRuntimeClient::new(RuntimeSnapshot::default()));
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut controller = TuiController::new(app, port, receiver);
    let start = Instant::now();
    let mut frames = FrameScheduler::default();
    frames.mark_drawn(start);
    controller.needs_redraw = false;
    let mut expected = String::new();
    let mut draws = 0;
    let area = Rect::new(0, 0, 100, 30);
    let mut buffer = Buffer::empty(area);

    for sequence in 1..=100 {
        let delta = format!("TOKEN-{sequence:03}\n");
        expected.push_str(&delta);
        assert!(
            controller.apply_runtime_event(RuntimeProjectionEvent::Runtime(Box::new(
                RuntimeControlEvent {
                    event_id: format!("event-{sequence}"),
                    provenance: RuntimeProvenance::local_tui("test-session"),
                    turn_id: Some("test-turn".into()),
                    sequence,
                    event: RuntimeEvent::Assistant(AssistantEvent::TextDelta(delta)),
                },
            )))
        );
        let now = start + Duration::from_micros(sequence);
        if std::mem::take(&mut controller.needs_redraw) {
            frames.request(now);
        }
        if frames.is_due(now) {
            let mut frame = Frame {
                cursor_position: None,
                viewport_area: area,
                buffer: &mut buffer,
            };
            crate::tui::render::render(&mut frame, controller.app_mut());
            frames.mark_drawn(now);
            draws += 1;
        }
    }
    assert_eq!(draws, 0);
    assert_eq!(
        controller
            .app()
            .agent_markdown_stream
            .as_ref()
            .unwrap()
            .raw_text,
        expected
    );
    assert!(
        controller.apply_runtime_event(RuntimeProjectionEvent::Runtime(Box::new(
            RuntimeControlEvent {
                event_id: "final-text".into(),
                provenance: RuntimeProvenance::local_tui("test-session"),
                turn_id: Some("test-turn".into()),
                sequence: 101,
                event: RuntimeEvent::Assistant(AssistantEvent::Text(expected.clone())),
            },
        )))
    );
    assert!(
        controller.apply_runtime_event(RuntimeProjectionEvent::Runtime(Box::new(
            RuntimeControlEvent {
                event_id: "turn-finished".into(),
                provenance: RuntimeProvenance::local_tui("test-session"),
                turn_id: Some("test-turn".into()),
                sequence: 102,
                event: RuntimeEvent::Session(SessionEvent::TurnFinished { reason: None }),
            },
        )))
    );
    frames.request(start + Duration::from_micros(102));
    let due = start + Duration::from_nanos(16_666_667);
    assert!(frames.is_due(due));
    let mut frame = Frame {
        cursor_position: None,
        viewport_area: area,
        buffer: &mut buffer,
    };
    crate::tui::render::render(&mut frame, controller.app_mut());
    frames.mark_drawn(due);
    draws += 1;
    assert_eq!(draws, 1);
    let text = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("TOKEN-100"),
        "the final delta must become visible"
    );
    assert!(!frames.is_due(due + Duration::from_secs(1)));
}
