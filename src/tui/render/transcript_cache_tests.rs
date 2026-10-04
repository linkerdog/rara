#![expect(
    clippy::print_stderr,
    reason = "Work-count regression tests expose measured counts in test diagnostics."
)]

use crate::tui::message_role::MessageRole;
use crate::tui::{
    selection::ScreenPosition,
    state::{RuntimePhase, RuntimeSnapshot, TranscriptEntry, TranscriptTurn},
    testing::TuiHarness,
    transcript_work::TranscriptWork,
};

fn history_harness() -> TuiHarness {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    let history = (0..1000)
        .map(|row| format!("HISTORY-{row:05}\n"))
        .collect::<String>();
    harness
        .app_mut()
        .restore_committed_turns(vec![TranscriptTurn {
            thinking_duration: None,
            entries: vec![TranscriptEntry::new(
                MessageRole::Agent,
                format!("```text\n{history}```"),
            )],
        }]);
    harness
}

#[test]
fn same_length_middle_edit_refreshes_production_selection() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    harness
        .app_mut()
        .push_entry(MessageRole::Agent, "```text\nfirst\nold\nlast\n```");
    let rows = super::renderable_transcript_lines(harness.app(), 80);
    let middle = rows
        .iter()
        .position(|line| line.to_string().trim() == "old")
        .expect("middle row");
    let text = &rows.get(middle).unwrap().text;
    let column = text.find("old").unwrap() as u16;
    let selection = &mut harness.app_mut().transcript_selection;
    let area = ratatui::layout::Rect::new(0, 0, 80, 20);
    selection.update_snapshot(&rows, area, 0);
    assert!(selection.start(ScreenPosition::new(column, middle as u16)));
    assert!(selection.drag(ScreenPosition::new(column + 3, middle as u16)));
    assert_eq!(selection.selected_text().as_deref(), Some("old"));
    harness.app_mut().active_turn.entries[0].message = "```text\nfirst\nnew\nlast\n```".into();
    let changed = super::renderable_transcript_lines(harness.app(), 80);
    assert_eq!(rows.len(), changed.len());
    assert_eq!(rows.get(0).unwrap().text, changed.get(0).unwrap().text);
    assert_eq!(
        rows.get(rows.len() - 1).unwrap().text,
        changed.get(changed.len() - 1).unwrap().text
    );
    harness
        .app_mut()
        .transcript_selection
        .update_snapshot(&changed, area, 0);
    assert_eq!(
        harness
            .app()
            .transcript_selection
            .selected_text()
            .as_deref(),
        Some("new")
    );
    assert_eq!(rows.get(middle).unwrap().text.trim(), "old");
}

fn work(harness: &TuiHarness) -> TranscriptWork {
    let render = harness.app().committed_render_cache.borrow().work.get();
    let selection = harness.app().transcript_selection.work.get();
    TranscriptWork {
        cloned_rows: render.cloned_rows + selection.cloned_rows,
        wrapped_lines: render.wrapped_lines + selection.wrapped_lines,
        text_rows: render.text_rows + selection.text_rows,
        hashed_rows: render.hashed_rows + selection.hashed_rows,
    }
}

#[test]
fn unchanged_frames_clone_only_visible_rows() {
    let mut harness = history_harness();
    harness.screen_buffer(80, 20);
    let before = work(&harness);
    for _ in 0..20 {
        harness.screen_buffer(80, 20);
    }
    let after = work(&harness);
    eprintln!("unchanged frames: {before:?} -> {after:?}");
    assert!(after.cloned_rows - before.cloned_rows <= 20 * 20);
    assert_eq!(after.wrapped_lines, before.wrapped_lines);
    assert_eq!(after.text_rows, before.text_rows);
    assert_eq!(after.hashed_rows, before.hashed_rows);
}

#[test]
fn scroll_input_reuses_materialized_history_without_wrapping() {
    let mut harness = history_harness();
    harness.screen_buffer(80, 20);
    let before = work(&harness);
    for _ in 0..20 {
        super::scroll_transcript(harness.app_mut(), -1);
        harness.screen_buffer(80, 20);
    }
    let after = work(&harness);
    eprintln!("scroll: {before:?} -> {after:?}");
    assert_eq!(after.wrapped_lines, before.wrapped_lines);
    assert_eq!(after.text_rows, before.text_rows);
}

#[test]
fn copy_and_selection_frames_do_not_hash_history() {
    let mut harness = history_harness();
    let (buffer, _) = harness.screen_buffer(80, 20);
    let tail = "HISTORY-00999";
    let (x, y) = (0..buffer.area.height)
        .find_map(|y| {
            let text = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>();
            text.find(tail).map(|x| (x as u16, y))
        })
        .expect("rendered tail");
    let selection = &mut harness.app_mut().transcript_selection;
    assert!(selection.start(ScreenPosition::new(x, y)));
    assert!(selection.drag(ScreenPosition::new(x + tail.len() as u16, y)));
    let before = work(&harness);
    for _ in 0..20 {
        harness.screen_buffer(80, 20);
        assert_eq!(
            harness
                .app()
                .transcript_selection
                .selected_text()
                .as_deref(),
            Some(tail)
        );
    }
    let after = work(&harness);
    eprintln!("selection: {before:?} -> {after:?}");
    assert_eq!(after.hashed_rows, before.hashed_rows);
    assert_eq!(after.text_rows, before.text_rows);
}

#[test]
fn streamed_frames_do_not_wrap_unchanged_committed_history() {
    let mut harness = history_harness();
    harness
        .app_mut()
        .push_entry(MessageRole::User, "Stream a small tail.");
    harness
        .app_mut()
        .set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    harness.screen_buffer(80, 20);
    let before = work(&harness);
    for _ in 0..50 {
        harness.app_mut().append_agent_delta("word ");
        harness.screen_buffer(80, 20);
    }
    let after = work(&harness);
    eprintln!("stream: {before:?} -> {after:?}");
    assert!(after.wrapped_lines - before.wrapped_lines <= 50 * 20);
    assert!(after.cloned_rows - before.cloned_rows <= 50 * 20);
    assert_eq!(after.hashed_rows, before.hashed_rows);
}

#[test]
fn committed_thinking_visibility_invalidates_the_layout_cache() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    harness
        .app_mut()
        .restore_committed_turns(vec![TranscriptTurn {
            thinking_duration: None,
            entries: vec![
                TranscriptEntry::new(MessageRole::Thinking, "Summary.\nPreview.\nTHOUGHT-DETAIL"),
                TranscriptEntry::new(MessageRole::Agent, "Final answer."),
            ],
        }]);
    harness.app_mut().thinking_collapsed = false;
    assert!(harness.screen_text(80, 30).contains("THOUGHT-DETAIL"));
    harness.app_mut().thinking_collapsed = true;
    assert!(!harness.screen_text(80, 30).contains("THOUGHT-DETAIL"));
}

#[test]
fn committed_appends_retain_prior_row_allocations() {
    let mut harness = history_harness();
    let original = super::renderable_transcript_lines(harness.app(), 80);
    let first = original.get(0).expect("history row");
    let text = first.text.as_ptr();
    let spans = first.line.spans.as_ptr();
    let before = work(&harness);
    for turn in 0..20 {
        harness
            .app_mut()
            .push_entry(MessageRole::User, format!("Turn {turn}"));
        harness
            .app_mut()
            .push_entry(MessageRole::Agent, "A short answer.");
        harness.app_mut().finalize_active_turn();
        let rows = super::renderable_transcript_lines(harness.app(), 80);
        assert_eq!(rows.get(0).expect("retained row").text.as_ptr(), text);
        assert_eq!(
            rows.get(0).expect("retained row").line.spans.as_ptr(),
            spans
        );
        assert!(rows.len() > original.len());
    }
    let after = work(&harness);
    assert!(after.wrapped_lines - before.wrapped_lines <= 20 * 20);
    assert_eq!(after.cloned_rows, before.cloned_rows);
    assert_eq!(after.hashed_rows, before.hashed_rows);
}

pub(super) fn canonical_rows(
    app: &crate::tui::state::TuiApp,
    width: u16,
) -> Vec<ratatui::text::Line<'static>> {
    let cwd = (!app.snapshot.cwd.is_empty()).then(|| std::path::Path::new(&app.snapshot.cwd));
    let mut logical = Vec::new();
    for turn in &app.committed_turns {
        let lines = super::committed_turn_lines(
            &turn.entries,
            cwd,
            width,
            app.thinking_collapsed,
            turn.thinking_duration,
        );
        if lines.is_empty() {
            continue;
        }
        if !logical.is_empty() {
            logical.push(super::turn_divider_line(width));
        }
        logical.extend(lines);
    }
    let active = super::active_turn_cell(app).display_lines(width);
    if !active.is_empty() {
        if !logical.is_empty() {
            logical.push(super::turn_divider_line(width));
        }
        logical.extend(active);
    }
    crate::tui::transcript_text::wrap_lines(&logical, width)
}

#[test]
fn layout_keys_reflow_width_and_cwd_but_reuse_height_changes() {
    let mut harness = history_harness();
    let initial = super::transcript_viewport(harness.app_mut(), 80, 20);
    let before = work(&harness);
    let taller = super::transcript_viewport(harness.app_mut(), 80, 40);
    assert!(std::ptr::eq(
        initial.lines.get(0).unwrap(),
        taller.lines.get(0).unwrap()
    ));
    assert_eq!(work(&harness), before);
    for width in [8, 12, 80, 120, 160] {
        let rows = super::renderable_transcript_lines(harness.app(), width);
        assert_eq!(
            rows.iter().cloned().collect::<Vec<_>>(),
            canonical_rows(harness.app(), width)
        );
    }
    let before_cwd = work(&harness);
    harness.app_mut().snapshot.cwd = "/workspace/changed".into();
    let rows = super::renderable_transcript_lines(harness.app(), 160);
    assert!(work(&harness).wrapped_lines > before_cwd.wrapped_lines);
    assert_eq!(
        rows.iter().cloned().collect::<Vec<_>>(),
        canonical_rows(harness.app(), 160)
    );
}

#[test]
fn active_and_committed_middle_replacements_refresh_copy_text() {
    use ratatui::layout::Rect;

    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    let old = "```text\nfirst\nold\nlast\n```";
    let new = "```text\nfirst\nnew\nlast\n```";
    harness.app_mut().push_entry(MessageRole::Agent, old);
    let rows = super::renderable_transcript_lines(harness.app(), 80);
    let middle = rows
        .iter()
        .position(|line| line.to_string().trim() == "old")
        .expect("middle row");
    let text = &rows.get(middle).unwrap().text;
    let start = crate::tui::text_wrap::display_width(&text[..text.find("old").unwrap()]) as u16;
    let area = Rect::new(0, 0, 80, 20);
    let selection = &mut harness.app_mut().transcript_selection;
    selection.update_snapshot(&rows, area, 0);
    assert!(selection.start(ScreenPosition::new(start, middle as u16)));
    assert!(selection.drag(ScreenPosition::new(start + 3, middle as u16)));
    assert_eq!(selection.selected_text().as_deref(), Some("old"));
    harness.app_mut().finalize_agent_stream(Some(new.into()));
    let replaced = super::renderable_transcript_lines(harness.app(), 80);
    assert_eq!(rows.len(), replaced.len());
    harness
        .app_mut()
        .transcript_selection
        .update_snapshot(&replaced, area, 0);
    assert_eq!(
        harness
            .app()
            .transcript_selection
            .selected_text()
            .as_deref(),
        Some("new")
    );

    harness.app_mut().finalize_active_turn();
    let committed = super::renderable_transcript_lines(harness.app(), 80);
    harness
        .app_mut()
        .transcript_selection
        .update_snapshot(&committed, area, 0);
    assert_eq!(
        harness
            .app()
            .transcript_selection
            .selected_text()
            .as_deref(),
        Some("new")
    );
    harness.app_mut().finalize_agent_stream(Some(old.into()));
    let corrected = super::renderable_transcript_lines(harness.app(), 80);
    harness
        .app_mut()
        .transcript_selection
        .update_snapshot(&corrected, area, 0);
    assert_eq!(
        harness
            .app()
            .transcript_selection
            .selected_text()
            .as_deref(),
        Some("old")
    );

    harness.app_mut().reset_transcript();
    let empty = super::renderable_transcript_lines(harness.app(), 80);
    harness
        .app_mut()
        .transcript_selection
        .update_snapshot(&empty, area, 0);
    assert_eq!(harness.app().transcript_selection.selected_text(), None);
    harness
        .app_mut()
        .restore_committed_turns(vec![TranscriptTurn {
            thinking_duration: None,
            entries: vec![TranscriptEntry::new(MessageRole::Agent, new)],
        }]);
    let restored = super::renderable_transcript_lines(harness.app(), 80);
    assert_eq!(
        restored.iter().cloned().collect::<Vec<_>>(),
        canonical_rows(harness.app(), 80)
    );
    assert_eq!(rows.get(middle).unwrap().text.trim(), "old");
}

#[test]
fn mixed_mutations_match_fresh_wrapping_and_preserve_old_snapshots() {
    let messages = [
        "# Heading\n\n**Styled** text with a [link](./src/main.rs).",
        "```rust\nlet value = 42;\n\n```",
        "- first\n- second\n\nWide \u{4e2d}\u{6587} and 👩‍💻 text.",
    ];
    for seed in [7_u32, 29, 113, 997] {
        let mut random = seed;
        let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
        let mut width = 80;
        let mut retained = super::renderable_transcript_lines(harness.app(), width);
        let mut retained_lines = Vec::new();
        for step in 0..120 {
            // Fixed seeds make the mixed sequence reproducible without a new dependency.
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let choice = (random >> 16) as usize;
            let message = messages[(choice / 9) % messages.len()];
            match choice % 9 {
                0 => {
                    harness
                        .app_mut()
                        .push_entry(MessageRole::User, format!("Question {step}"));
                    harness
                        .app_mut()
                        .push_entry(MessageRole::Thinking, "Summary.\nPreview.\nDetail.");
                    harness.app_mut().push_entry(MessageRole::Agent, message);
                }
                1 => harness.app_mut().finalize_active_turn(),
                2 => harness
                    .app_mut()
                    .finalize_agent_stream(Some(message.into())),
                3 => harness.app_mut().thinking_collapsed = !harness.app().thinking_collapsed,
                4 => harness.app_mut().snapshot.cwd = format!("/workspace/{step}"),
                5 => harness
                    .app_mut()
                    .restore_committed_turns(vec![TranscriptTurn {
                        thinking_duration: None,
                        entries: vec![TranscriptEntry::new(MessageRole::Agent, message)],
                    }]),
                6 => harness.app_mut().reset_transcript(),
                7 => harness
                    .app_mut()
                    .append_agent_delta("Another **streamed** paragraph.\n\n"),
                8 => width = [1, 2, 8, 20, 80, 120][(choice / 9) % 6],
                _ => unreachable!("bounded mutation choice"),
            }
            let rows = super::renderable_transcript_lines(harness.app(), width);
            assert_eq!(
                rows.iter().cloned().collect::<Vec<_>>(),
                canonical_rows(harness.app(), width),
                "seed {seed}, step {step}, width {width}"
            );
            assert_eq!(retained.iter().cloned().collect::<Vec<_>>(), retained_lines);
            if step % 11 == 0 {
                retained_lines = rows.iter().cloned().collect();
                retained = rows;
            }
        }
    }
}
