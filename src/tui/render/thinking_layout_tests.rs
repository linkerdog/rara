use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::tui::{
    message_role::MessageRole,
    selection::ScreenPosition,
    state::{
        AgentMarkdownStreamState, RuntimePhase, RuntimeSnapshot, TranscriptEntry, TranscriptTurn,
    },
    testing::TuiHarness,
};

fn thinking_with_static_sections() -> TuiHarness {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let app = harness.app_mut();
    app.push_entry(MessageRole::User, "RETAINED PROMPT");
    app.push_entry(MessageRole::Agent, "EARLIER ANSWER");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.snapshot
        .plan_steps
        .push(("pending".into(), "RETAINED SUFFIX".into()));
    app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    app.append_agent_thinking_delta("First evidence.\n\nOld evidence.");
    app.active_live.thinking_started_at = None;
    harness
}

#[test]
fn thinking_slot_retains_both_static_sections_and_cross_section_copy() {
    let mut harness = thinking_with_static_sections();
    let before = super::renderable_transcript_lines(harness.app(), 80);
    let prompt = before
        .iter()
        .position(|line| line.to_string().contains("RETAINED PROMPT"))
        .unwrap();
    let suffix = before
        .iter()
        .position(|line| line.to_string().contains("RETAINED SUFFIX"))
        .unwrap();
    harness
        .app_mut()
        .append_agent_thinking_delta("\n\nNew evidence.");
    let after = super::renderable_transcript_lines(harness.app(), 80);
    let new_suffix = after
        .iter()
        .position(|line| line.to_string().contains("RETAINED SUFFIX"))
        .unwrap();
    let thinking = after
        .iter()
        .position(|line| line.to_string().contains("New evidence."))
        .unwrap();
    assert!(prompt < thinking && thinking < new_suffix);
    assert!(std::ptr::eq(
        before.get(prompt).unwrap(),
        after.get(prompt).unwrap()
    ));
    assert!(std::ptr::eq(
        before.get(suffix).unwrap(),
        after.get(new_suffix).unwrap()
    ));
    assert_eq!(
        after.iter().cloned().collect::<Vec<_>>(),
        super::transcript_cache_tests::canonical_rows(harness.app(), 80)
    );
    assert!(
        !before
            .iter()
            .any(|line| line.to_string().contains("New evidence."))
    );
    let expected = after
        .iter()
        .skip(prompt)
        .take(new_suffix - prompt + 1)
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let selection = &mut harness.app_mut().transcript_selection;
    selection.update_snapshot(&after, Rect::new(0, 0, 80, 40), 0);
    assert!(selection.start(ScreenPosition::new(0, prompt as u16)));
    assert!(selection.drag(ScreenPosition::new(
        after.get(new_suffix).unwrap().width as u16,
        new_suffix as u16
    )));
    assert_eq!(
        selection.selected_text().as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn thinking_slot_empty_visibility_history_and_lifecycle_match_cold_rows() {
    for with_history in [false, true] {
        let mut harness = thinking_with_static_sections();
        let app = harness.app_mut();
        if with_history {
            app.restore_committed_turns(vec![TranscriptTurn {
                entries: vec![TranscriptEntry::new(MessageRole::Agent, "History.")],
                thinking_duration: None,
            }]);
            app.push_entry(MessageRole::User, "RETAINED PROMPT");
            app.push_entry(MessageRole::Agent, "EARLIER ANSWER");
            app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
        }
        for source in [
            "",
            "\n\n",
            "Visible **evidence**.",
            "",
            "```rust\nlet x = 1;\n",
            "Final evidence.",
        ] {
            let mut stream = AgentMarkdownStreamState::new(PathBuf::from("/workspace"));
            stream.push_delta(source);
            app.agent_thinking_stream = Some(stream);
            app.active_live.thinking_started_at = None;
            for width in [80, 8, 1, 0, 80] {
                let actual = super::renderable_transcript_lines(app, width);
                assert_eq!(
                    actual.iter().cloned().collect::<Vec<_>>(),
                    super::transcript_cache_tests::canonical_rows(app, width),
                    "source={source:?}, width={width}, history={with_history}"
                );
            }
        }
        app.append_agent_delta("A streaming response.");
        let actual = super::renderable_transcript_lines(app, 80);
        assert_eq!(
            actual.iter().cloned().collect::<Vec<_>>(),
            super::transcript_cache_tests::canonical_rows(app, 80)
        );
        app.finalize_agent_stream(None);
        app.finalize_active_turn();
        let actual = super::renderable_transcript_lines(app, 80);
        assert_eq!(
            actual.iter().cloned().collect::<Vec<_>>(),
            super::transcript_cache_tests::canonical_rows(app, 80)
        );
    }
}

#[test]
fn thinking_slot_handles_no_prompt_or_suffix_and_borrowed_source_lines() {
    let mut harness = thinking_with_static_sections();
    let app = harness.app_mut();
    app.active_turn.entries.clear();
    app.push_entry(MessageRole::Thinking, "Previous reasoning.");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Execute;
    app.snapshot.plan_steps.clear();
    assert_eq!(
        super::active_turn_cell(app)
            .shared_layout(80)
            .after_thinking,
        Some(Vec::new())
    );
    let source = app.agent_thinking_stream_lines().unwrap();
    for width in [80, 1, 0, 80] {
        let actual = super::renderable_transcript_lines(app, width);
        assert_eq!(
            actual.iter().cloned().collect::<Vec<_>>(),
            super::transcript_cache_tests::canonical_rows(app, width)
        );
        assert!(!source.is_empty());
    }
}
