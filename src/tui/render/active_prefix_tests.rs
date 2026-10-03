use crate::{
    runtime_control::{AssistantEvent, RuntimeControlEvent, RuntimeEvent, RuntimeProvenance},
    tui::{
        message_role::MessageRole,
        state::{RuntimePhase, RuntimeSnapshot, TuiEvent},
        testing::TuiHarness,
    },
};

fn long_prefix() -> TuiHarness {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let app = harness.app_mut();
    app.push_entry(
        MessageRole::User,
        "Inspect the entire document.\n".repeat(1000),
    );
    app.push_entry(
        MessageRole::Thinking,
        "A completed reasoning paragraph.\n\n".repeat(1000),
    );
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("streaming model output".into()),
    );
    harness
}

#[test]
fn unchanged_prefix_skips_assembly_on_frames_scroll_and_composer_edits() {
    let mut harness = long_prefix();
    harness.app_mut().push_entry(
        MessageRole::Agent,
        "A full non-streaming answer.\n\n".repeat(1000),
    );
    harness.screen_buffer(80, 20);
    let before = harness.app().active_assembly_count.get();
    for _ in 0..20 {
        harness.app_mut().bottom_pane.input.push('x');
        super::scroll_transcript(harness.app_mut(), -1);
        harness.screen_buffer(80, 20);
    }
    assert_eq!(harness.app().active_assembly_count.get(), before);
}

#[test]
fn response_deltas_do_not_reassemble_the_long_prefix() {
    let mut harness = long_prefix();
    harness.app_mut().append_agent_delta("First paragraph.\n\n");
    harness.screen_buffer(80, 20);
    let before = harness.app().active_assembly_count.get();
    for sequence in 0..30 {
        crate::tui::runtime::apply_tui_event(
            harness.app_mut(),
            TuiEvent::Runtime(Box::new(RuntimeControlEvent {
                event_id: format!("prefix-{sequence}"),
                provenance: RuntimeProvenance::local_tui("prefix-session"),
                turn_id: Some("prefix-turn".into()),
                sequence,
                event: RuntimeEvent::Assistant(AssistantEvent::TextDelta("More text.\n\n".into())),
            })),
        );
        harness.screen_buffer(80, 20);
    }
    assert_eq!(harness.app().active_assembly_count.get(), before);
}
