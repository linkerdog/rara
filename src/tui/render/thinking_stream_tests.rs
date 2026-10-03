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
        assert!(
            meter.get().cloned_rows - before.cloned_rows <= 20 * 4,
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

fn growing_thinking_with_long_prefix() -> TuiHarness {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let app = harness.app_mut();
    app.push_entry(MessageRole::User, "A long prompt line.\n".repeat(1000));
    app.push_entry(
        MessageRole::Agent,
        "A static answer paragraph.\n\n".repeat(1000),
    );
    app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    app.append_agent_thinking_delta("```text\nfirst\nsecond\nthird\nfourth\nfifth\n");
    app.active_live.thinking_started_at = None;
    harness.screen_buffer(80, 20);
    harness
}

#[test]
fn thinking_updates_do_not_reassemble_static_sections() {
    let mut harness = growing_thinking_with_long_prefix();
    let before = harness.app().active_assembly_count.get();
    for index in 0..20 {
        harness
            .app_mut()
            .append_agent_thinking_delta(&format!("THINK-{index:05}\n"));
        harness.screen_buffer(80, 20);
    }
    assert_eq!(harness.app().active_assembly_count.get(), before);
}

#[test]
fn thinking_updates_wrap_only_the_visible_window() {
    let mut harness = growing_thinking_with_long_prefix();
    let meter = harness.app().committed_render_cache.borrow().work.clone();
    let before = meter.get();
    for index in 0..20 {
        harness
            .app_mut()
            .append_agent_thinking_delta(&format!("THINK-{index:05}\n"));
        harness.screen_buffer(80, 20);
    }
    assert!(
        meter.get().wrapped_lines - before.wrapped_lines <= 20 * 6,
        "thinking updates wrapped {} logical lines",
        meter.get().wrapped_lines - before.wrapped_lines
    );
}

#[test]
fn thinking_updates_retain_a_large_plan_suffix() {
    let mut harness = growing_thinking_with_long_prefix();
    let app = harness.app_mut();
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.snapshot
        .plan_steps
        .extend((0..1000).map(|index| ("pending".into(), format!("Plan step {index}"))));
    harness.screen_buffer(80, 20);
    let before_assembly = harness.app().active_assembly_count.get();
    let meter = harness.app().committed_render_cache.borrow().work.clone();
    let before = meter.get();
    for index in 0..20 {
        harness
            .app_mut()
            .append_agent_thinking_delta(&format!("THINK-{index:05}\n"));
        harness.screen_buffer(80, 20);
    }
    assert_eq!(harness.app().active_assembly_count.get(), before_assembly);
    assert!(meter.get().wrapped_lines - before.wrapped_lines <= 20 * 6);
}
