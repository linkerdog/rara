use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

use super::TranscriptViewport;
use crate::tui::custom_terminal::Frame;
use crate::tui::message_role::MessageRole;
use crate::tui::selection::{ScreenPosition, TranscriptSelection};
use crate::tui::text_wrap::display_width;
use crate::tui::theme::TEXT_ACCENT;

fn viewport_buffer(viewport: &TranscriptViewport, area: Rect) -> Buffer {
    let mut buffer = Buffer::empty(area);
    viewport.render(
        &mut Frame {
            cursor_position: None,
            viewport_area: area,
            buffer: &mut buffer,
        },
        area,
    );
    buffer
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (buffer.area.x..buffer.area.right())
        .map(|x| buffer[(x, y)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn word_wrapped_rows_are_counted_exactly() {
    let lines = vec![Line::from("aaaa bbbb cccc dddd")];
    let viewport = TranscriptViewport::new(lines.clone(), 0, 8);
    let buffer = viewport_buffer(&viewport, Rect::new(0, 0, 8, 5));
    assert_eq!(
        (0..4).map(|y| row_text(&buffer, y)).collect::<Vec<_>>(),
        ["aaaa", "bbbb", "cccc", "dddd"]
    );
    assert_eq!(viewport.lines.len(), 4);
}

#[test]
fn word_wrapped_tail_window_retains_the_last_prose_row_and_next_line() {
    let viewport = TranscriptViewport::new(
        vec![Line::from("aaaa bbbb cccc dddd"), Line::from("TAIL")],
        3,
        8,
    );
    let buffer = viewport_buffer(&viewport, Rect::new(0, 0, 8, 2));
    assert_eq!(row_text(&buffer, 0), "dddd");
    assert_eq!(row_text(&buffer, 1), "TAIL");
}

#[test]
fn selecting_a_word_wrapped_row_copies_the_highlighted_cells() {
    let viewport = TranscriptViewport::new(vec![Line::from("aaaa bbbb cccc dddd")], 0, 8);
    let area = Rect::new(2, 3, 8, 4);
    let mut selection = TranscriptSelection::default();
    selection.update_snapshot(&viewport.lines, area, 0);
    assert!(selection.start(ScreenPosition::new(2, 4)));
    assert!(selection.drag(ScreenPosition::new(6, 4)));
    let mut buffer = viewport_buffer(&viewport, area);
    selection.highlight_visible_range(&mut buffer);
    let highlighted = (area.x..area.right())
        .filter(|x| buffer[(*x, 4)].modifier.contains(Modifier::REVERSED))
        .map(|x| buffer[(x, 4)].symbol())
        .collect::<String>();
    assert_eq!(highlighted, "bbbb");
    assert_eq!(
        selection.selected_text().as_deref(),
        Some(highlighted.as_str())
    );
}

#[test]
fn counted_rendered_and_selected_rows_agree_across_the_width_matrix() {
    let fixtures = [
        "aaaa bbbb cccc dddd",
        "\u{754c}\u{6587} alpha \u{8bed}\u{8a00}",
        "a\u{301}b\u{301} \u{1f469}\u{200d}\u{1f4bb} \u{1f1f8}\u{1f1ec} end",
        "https://example.test/a-very-long-token?q=alpha-beta",
        "a\tb\n\nTAIL\n",
    ];
    for width in [1, 2, 3, 4, 8, 12, 80, 120, 160] {
        for input in fixtures {
            let viewport = TranscriptViewport::new(vec![Line::from(input)], 0, width);
            let height = u16::try_from(viewport.lines.len() + 1).expect("fixture height");
            let area = Rect::new(2, 3, width, height);
            let buffer = viewport_buffer(&viewport, area);
            let mut selection = TranscriptSelection::default();
            selection.update_snapshot(&viewport.lines, area, 0);
            for (row, line) in viewport.lines.iter().enumerate() {
                let expected = line.to_string();
                let mut col = area.x;
                for grapheme in expected.graphemes(true) {
                    assert_eq!(
                        buffer[(col, area.y + row as u16)].symbol(),
                        grapheme,
                        "{input:?}, width {width}"
                    );
                    col += display_width(grapheme) as u16;
                }
                assert!(usize::from(col - area.x) <= usize::from(width));
                if col > area.x {
                    assert!(selection.start(ScreenPosition::new(area.x, area.y + row as u16)));
                    assert!(selection.drag(ScreenPosition::new(col, area.y + row as u16)));
                    assert_eq!(
                        selection.selected_text().as_deref(),
                        Some(expected.as_str()),
                        "{input:?}, width {width}"
                    );
                    selection.clear();
                }
            }
            assert_eq!(row_text(&buffer, area.y + height - 1), "");
        }
    }
}

#[test]
fn selection_highlight_and_copy_snap_to_an_entire_wide_grapheme() {
    let emoji = "\u{1f469}\u{200d}\u{1f4bb}";
    let viewport = TranscriptViewport::new(vec![Line::from(format!("{emoji}x"))], 0, 4);
    let area = Rect::new(0, 0, 4, 1);
    let mut selection = TranscriptSelection::default();
    selection.update_snapshot(&viewport.lines, area, 0);
    assert!(selection.start(ScreenPosition::new(1, 0)));
    assert!(selection.drag(ScreenPosition::new(2, 0)));
    let mut buffer = viewport_buffer(&viewport, area);
    selection.highlight_visible_range(&mut buffer);
    assert_eq!(selection.selected_text().as_deref(), Some(emoji));
    assert!(buffer[(0, 0)].modifier.contains(Modifier::REVERSED));
    assert!(buffer[(1, 0)].modifier.contains(Modifier::REVERSED));
    assert!(!buffer[(2, 0)].modifier.contains(Modifier::REVERSED));
}

#[test]
fn styled_span_boundaries_do_not_split_clusters_or_lose_style_on_wrap() {
    let viewport = TranscriptViewport::new(
        vec![
            Line::from(vec![
                Span::styled("a", Style::default().fg(TEXT_ACCENT)),
                Span::raw("\u{301} bbbb cccc"),
            ])
            .style(Style::default().add_modifier(Modifier::ITALIC)),
        ],
        0,
        4,
    );
    assert_eq!(
        viewport
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>(),
        ["a\u{301}", "bbbb", "cccc"]
    );
    let buffer = viewport_buffer(&viewport, Rect::new(0, 0, 4, 3));
    assert_eq!(buffer[(0, 0)].symbol(), "a\u{301}");
    assert_eq!(buffer[(0, 0)].fg, TEXT_ACCENT);
    for y in 0..3 {
        assert!(buffer[(0, y)].modifier.contains(Modifier::ITALIC));
    }
}

#[test]
fn alignment_is_materialized_for_selection_and_partial_window_rendering() {
    let viewport = TranscriptViewport::new(
        vec![
            Line::from("abc").alignment(Alignment::Right),
            Line::from("middle").alignment(Alignment::Center),
            Line::from("tail"),
        ],
        1,
        8,
    );
    let area = Rect::new(0, 0, 8, 2);
    let mut selection = TranscriptSelection::default();
    selection.update_snapshot(&viewport.lines, area, 1);
    let buffer = viewport_buffer(&viewport, area);
    assert_eq!(row_text(&buffer, 0), " middle");
    assert!(selection.start(ScreenPosition::new(1, 0)));
    assert!(selection.drag(ScreenPosition::new(7, 0)));
    assert_eq!(selection.selected_text().as_deref(), Some("middle"));
    assert_eq!(row_text(&buffer, 1), "tail");
    assert_eq!(viewport.lines[0].to_string(), "     abc");
}

#[test]
fn full_renderer_follow_tail_keeps_final_word_and_updates_selection() {
    use crate::tui::{
        state::{RuntimeSnapshot, TranscriptEntry, TranscriptTurn},
        testing::TuiHarness,
    };

    for width in [8, 12, 80, 120, 160] {
        let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
        harness
            .app_mut()
            .restore_committed_turns(vec![TranscriptTurn {
                thinking_duration: None,
                entries: vec![TranscriptEntry {
                    role: MessageRole::Agent,
                    message: format!("{} TAIL", "aaaa bbbb cccc dddd ".repeat(10)),
                    payload: None,
                }],
            }]);
        let (buffer, _) = harness.screen_buffer(width, 20);
        let (x, y) = (0..20)
            .find_map(|y| {
                let row = row_text(&buffer, y);
                row.find("TAIL")
                    .map(|offset| (display_width(&row[..offset]) as u16, y))
            })
            .expect("final word visible at follow-tail");
        let selection = &mut harness.app_mut().transcript_selection;
        assert!(selection.start(ScreenPosition::new(x, y)), "width {width}");
        assert!(selection.drag(ScreenPosition::new(x + 4, y)));
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("TAIL"),
            "width {width}"
        );
    }
}
