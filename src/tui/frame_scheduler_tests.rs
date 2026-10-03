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
