use std::time::{Duration, Instant};

use crate::tui::{
    message_role::MessageRole,
    state::{RuntimePhase, RuntimeSnapshot},
    testing::TuiHarness,
};

#[test]
fn thinking_clock_invalidates_only_when_the_visible_duration_changes() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let app = harness.app_mut();
    app.push_entry(MessageRole::User, "Explain the evidence.");
    app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    app.append_agent_thinking_delta("The evidence agrees.");
    let start = Instant::now();
    app.active_live.thinking_started_at = Some(start);
    let render_at = |millis| {
        super::materialize_cell(
            app,
            80,
            super::ActiveTurnCell::at_time(app, None, start + Duration::from_millis(millis)),
        )
    };
    let first = render_at(1000);
    let before = app.active_assembly_count.get();
    let same = render_at(1040);
    assert_eq!(app.active_assembly_count.get(), before);
    assert!(std::ptr::eq(first.get(0).unwrap(), same.get(0).unwrap()));
    let changed = render_at(1060);
    assert_eq!(app.active_assembly_count.get(), before + 1);
    assert!(
        first
            .iter()
            .any(|row| row.to_string().contains("Thinking (1.0s)"))
    );
    assert!(
        changed
            .iter()
            .any(|row| row.to_string().contains("Thinking (1.1s)"))
    );
}
