use crate::tui::{
    message_role::MessageRole,
    state::{RuntimePhase, RuntimeSnapshot},
    testing::TuiHarness,
};

#[test]
fn live_thinking_frames_copy_only_the_selected_tail() {
    for row_count in [32, 4096] {
        let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
        let app = harness.app_mut();
        app.push_entry(MessageRole::User, "Inspect the evidence.");
        app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
        app.append_agent_thinking_delta(&format!(
            "```text\n{}",
            (0..row_count)
                .map(|index| format!("THINK-{index:05}\n"))
                .collect::<String>()
        ));
        harness.screen_buffer(80, 20);
        let stream = harness.app().agent_thinking_stream.as_ref().unwrap();
        let meter = stream.layout_work();
        let source_work = stream.markdown_work();
        let before = meter.get();
        for _ in 0..20 {
            harness.screen_buffer(80, 20);
        }
        assert_eq!(
            meter.get().cloned_rows - before.cloned_rows,
            20 * 4,
            "copy cost must be independent of {row_count} source rows"
        );
        assert_eq!(
            harness
                .app()
                .agent_thinking_stream
                .as_ref()
                .unwrap()
                .markdown_work(),
            source_work
        );
    }
}

#[test]
fn growing_thinking_stream_copies_only_four_rows_per_frame() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let app = harness.app_mut();
    app.push_entry(MessageRole::User, "Inspect the evidence.");
    app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    app.append_agent_thinking_delta("```text\nfirst\nsecond\nthird\nfourth\n");
    harness.screen_buffer(80, 20);
    let meter = harness
        .app()
        .agent_thinking_stream
        .as_ref()
        .unwrap()
        .layout_work();
    let before = meter.get();
    for index in 0..200 {
        harness
            .app_mut()
            .append_agent_thinking_delta(&format!("THINK-{index:05}\n"));
        harness.screen_buffer(80, 20);
    }
    assert_eq!(meter.get().cloned_rows - before.cloned_rows, 200 * 4);
}
