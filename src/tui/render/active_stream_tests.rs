use crate::tui::{
    state::{RuntimePhase, RuntimeSnapshot},
    testing::TuiHarness,
    transcript_work::TranscriptWork,
};

fn stream_harness() -> TuiHarness {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    harness.app_mut().push_entry("You", "Stream a long answer.");
    harness
        .app_mut()
        .set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    harness
}

fn work(harness: &TuiHarness) -> TranscriptWork {
    let render = harness.app().committed_render_cache.borrow().work.get();
    let stream = harness
        .app()
        .agent_markdown_stream
        .as_ref()
        .unwrap()
        .layout_work()
        .get();
    TranscriptWork {
        cloned_rows: render.cloned_rows + stream.cloned_rows,
        wrapped_lines: render.wrapped_lines + stream.wrapped_lines,
        text_rows: render.text_rows + stream.text_rows,
        hashed_rows: render.hashed_rows + stream.hashed_rows,
    }
}

#[test]
fn unchanged_long_stream_frames_clone_only_visible_rows() {
    let mut harness = stream_harness();
    harness
        .app_mut()
        .append_agent_delta(&"A stable paragraph.\n\n".repeat(1000));
    harness.screen_buffer(80, 20);
    let before = work(&harness);
    for _ in 0..20 {
        harness.screen_buffer(80, 20);
    }
    let after = work(&harness);
    eprintln!("long unchanged: {before:?} -> {after:?}");
    assert!(after.cloned_rows - before.cloned_rows <= 20 * 20);
    assert_eq!(after.wrapped_lines, before.wrapped_lines);
    assert_eq!(after.text_rows, before.text_rows);
}

#[test]
fn growing_paragraph_stream_wraps_only_new_and_mutable_rows() {
    let mut harness = stream_harness();
    for row in 0..200 {
        harness
            .app_mut()
            .append_agent_delta(&format!("Paragraph {row:05}.\n\n"));
        harness.screen_buffer(80, 20);
    }
    let work = work(&harness);
    eprintln!("paragraph growth: {work:?}");
    assert!(work.wrapped_lines <= 200 * 20);
    assert!(work.cloned_rows <= 200 * 40);
}

#[test]
fn growing_open_fence_wraps_only_appended_code_rows() {
    let mut harness = stream_harness();
    harness.app_mut().append_agent_delta("```rust\n");
    for row in 0..200 {
        harness
            .app_mut()
            .append_agent_delta(&format!("let row_{row} = {row};\n"));
        harness.screen_buffer(80, 20);
    }
    let work = work(&harness);
    eprintln!("fence growth: {work:?}");
    assert!(work.wrapped_lines <= 200 * 20);
    assert!(work.cloned_rows <= 200 * 40);
}

#[test]
fn growing_stream_retains_stable_styled_row_allocations() {
    let mut harness = stream_harness();
    harness
        .app_mut()
        .append_agent_delta("FIRST-STABLE.\n\nMutable paragraph.\n\n");
    let initial = super::renderable_transcript_lines(harness.app(), 80);
    let index = initial
        .iter()
        .position(|line| line.to_string().contains("FIRST-STABLE"))
        .unwrap();
    for _ in 0..20 {
        harness
            .app_mut()
            .append_agent_delta("Another paragraph.\n\n");
        let rows = super::renderable_transcript_lines(harness.app(), 80);
        assert!(std::ptr::eq(
            initial.get(index).unwrap(),
            rows.get(index).unwrap()
        ));
    }
}

fn assert_canonical(harness: &TuiHarness, width: u16) {
    let rows = super::renderable_transcript_lines(harness.app(), width);
    assert_eq!(
        rows.iter().cloned().collect::<Vec<_>>(),
        super::transcript_cache_tests::canonical_rows(harness.app(), width)
    );
}

#[test]
fn shared_stream_matches_inline_cells_across_prefix_and_compact_transitions() {
    let mut harness = stream_harness();
    for chunk in [
        "# Answer\n\n",
        "**Styled** \u{4e2d}\u{6587} 👩‍💻.\n\n",
        "```rust\nlet first = 1;\n",
        "let next = 2;\n",
        "```\n\nDone.",
    ] {
        harness.app_mut().append_agent_delta(chunk);
        for width in [1, 2, 8, 80, 120, 160] {
            assert_canonical(&harness, width);
        }
    }
    let retained = super::renderable_transcript_lines(harness.app(), 80);
    harness
        .app_mut()
        .active_live
        .running_actions
        .push("Run verification".into());
    assert_eq!(
        super::active_turn_cell(harness.app())
            .shared_layout(80)
            .stream,
        Some(super::ResponseView::Compact)
    );
    assert_canonical(&harness, 80);
    harness.app_mut().active_live.running_actions[0] = "Verify changed result".into();
    assert_canonical(&harness, 80);
    harness.app_mut().active_live.running_actions.clear();
    assert_eq!(
        super::active_turn_cell(harness.app())
            .shared_layout(80)
            .stream,
        Some(super::ResponseView::Full)
    );
    assert_canonical(&harness, 80);
    assert!(
        retained
            .iter()
            .any(|line| line.to_string().contains("Done."))
    );
}

#[test]
fn stream_suppression_thinking_and_finalization_keep_canonical_cell_order() {
    let mut harness = stream_harness();
    harness
        .app_mut()
        .append_agent_thinking_delta("Thought one.\nThought two.\nHidden detail.");
    assert_canonical(&harness, 80);
    harness
        .app_mut()
        .append_agent_delta("Answer one.\n\nAnswer two.\n\n");
    for collapsed in [true, false, true] {
        harness.app_mut().thinking_collapsed = collapsed;
        assert_canonical(&harness, 80);
    }
    harness.app_mut().push_entry("Tool", "bash: cargo check");
    harness
        .app_mut()
        .set_runtime_phase(RuntimePhase::RunningTool, None);
    assert!(
        super::active_turn_cell(harness.app())
            .shared_layout(80)
            .stream
            .is_none()
    );
    assert_canonical(&harness, 80);
    harness
        .app_mut()
        .set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    assert_canonical(&harness, 80);
    harness
        .app_mut()
        .finalize_agent_stream(Some("Final replacement.".into()));
    assert_canonical(&harness, 80);
    harness.app_mut().finalize_active_turn();
    assert_canonical(&harness, 80);
    harness.app_mut().push_entry("You", "Next turn.");
    harness.app_mut().append_agent_delta("Next answer.");
    assert_canonical(&harness, 80);
    harness.app_mut().reset_transcript();
    assert_canonical(&harness, 80);
}

#[test]
fn unchanged_long_stream_scroll_and_copy_reuse_shared_rows() {
    use crate::tui::selection::ScreenPosition;

    let mut harness = stream_harness();
    harness.app_mut().append_agent_delta("```text\n");
    for row in 0..1000 {
        harness
            .app_mut()
            .append_agent_delta(&format!("STREAM-{row:05}\n"));
    }
    let (buffer, _) = harness.screen_buffer(80, 20);
    let needle = "STREAM-00999";
    let (x, y) = (0..buffer.area.height)
        .find_map(|y| {
            let text = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>();
            text.find(needle).map(|x| (x as u16, y))
        })
        .expect("visible stream tail");
    let selection = &mut harness.app_mut().transcript_selection;
    assert!(selection.start(ScreenPosition::new(x, y)));
    assert!(selection.drag(ScreenPosition::new(x + needle.len() as u16, y)));
    let before = work(&harness);
    for _ in 0..20 {
        harness.screen_buffer(80, 20);
        assert_eq!(
            harness
                .app()
                .transcript_selection
                .selected_text()
                .as_deref(),
            Some(needle)
        );
    }
    for _ in 0..20 {
        super::scroll_transcript(harness.app_mut(), -1);
        harness.screen_buffer(80, 20);
    }
    let after = work(&harness);
    assert_eq!(after.wrapped_lines, before.wrapped_lines);
    assert_eq!(after.text_rows, before.text_rows);
    assert!(after.cloned_rows - before.cloned_rows <= 40 * 20);
    assert_eq!(harness.app().transcript_selection.work.get().hashed_rows, 0);
}

#[test]
fn changing_preview_refreshes_selected_text_without_mutating_retained_rows() {
    use crate::tui::selection::ScreenPosition;

    let mut harness = stream_harness();
    harness.app_mut().append_agent_delta("Stable.\n\nmutable");
    let old = super::renderable_transcript_lines(harness.app(), 80);
    let index = old
        .iter()
        .position(|line| line.to_string().contains("mutable"))
        .unwrap();
    let text = &old.get(index).unwrap().text;
    let column = text.find("mutable").unwrap() as u16;
    let area = ratatui::layout::Rect::new(0, 0, 80, 20);
    let selection = &mut harness.app_mut().transcript_selection;
    selection.update_snapshot(&old, area, 0);
    assert!(selection.start(ScreenPosition::new(column, index as u16)));
    assert!(selection.drag(ScreenPosition::new(column + 9, index as u16)));
    assert_eq!(selection.selected_text().as_deref(), Some("mutable"));
    harness.app_mut().append_agent_delta("++");
    let changed = super::renderable_transcript_lines(harness.app(), 80);
    harness
        .app_mut()
        .transcript_selection
        .update_snapshot(&changed, area, 0);
    // Selection coordinates do not automatically extend when a line grows.
    assert_eq!(
        harness
            .app()
            .transcript_selection
            .selected_text()
            .as_deref(),
        Some("mutable")
    );
    assert!(
        harness
            .app_mut()
            .transcript_selection
            .drag(ScreenPosition::new(column + 9, index as u16,))
    );
    assert_eq!(
        harness
            .app()
            .transcript_selection
            .selected_text()
            .as_deref(),
        Some("mutable++")
    );
    assert_eq!(old.get(index).unwrap().text, *text);
    assert!(old.get(index).unwrap().text.ends_with("mutable"));
    assert_canonical(&harness, 80);
}
