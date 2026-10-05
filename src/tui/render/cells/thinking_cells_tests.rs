use std::time::Duration;

use ratatui::style::{Color, Modifier};

use super::*;
use crate::tui::transcript_work::WorkMeter;

#[test]
fn live_tail_preserves_dimmed_span_styles_and_duration_chrome() {
    let source = vec![
        Line::from("hidden first"),
        Line::from("hidden second"),
        Line::from(vec![
            Span::styled("bold", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(" colored", Style::default().fg(Color::Red).bg(Color::Blue)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "italic",
            Style::default().add_modifier(Modifier::ITALIC),
        )),
        Line::from("last"),
    ];
    let work = WorkMeter::default();
    let cell = ThinkingBlockCell::from_stream(&source, Some(Duration::from_millis(1250)))
        .with_work_meter(work.clone());
    let muted = Style::default().fg(TEXT_MUTED);
    assert_eq!(
        cell.display_lines(80),
        vec![
            Line::from(Span::styled("┊ Thinking (1.2s) — Alt+T to collapse", muted)),
            Line::from(Span::styled("┊  ... 2 more lines", muted)),
            Line::from(vec![
                Span::styled("┊ ", muted),
                Span::styled("bold", muted.add_modifier(Modifier::BOLD)),
                Span::styled(" colored", muted.bg(Color::Blue)),
            ]),
            Line::from(Span::styled("┊ ", muted)),
            Line::from(vec![
                Span::styled("┊ ", muted),
                Span::styled("italic", muted.add_modifier(Modifier::ITALIC)),
            ]),
            Line::from(vec![Span::styled("┊ ", muted), Span::styled("last", muted)]),
        ]
    );
    assert_eq!(work.get().cloned_rows, 4);
    assert_eq!(source[0], Line::from("hidden first"));
    assert_eq!(source[2].spans[1].style.fg, Some(Color::Red));
}

#[test]
fn committed_thinking_keeps_head_tail_and_empty_contracts() {
    let message = "```text\nfirst\nsecond\nthird\nfourth\nfifth\n```";
    let expanded = ThinkingBlockCell::new(message, 4, false, None).display_lines(80);
    assert_eq!(
        expanded.iter().map(ToString::to_string).collect::<Vec<_>>(),
        [
            "┊ Thinking",
            "┊  ... 2 more lines",
            "┊ second",
            "┊ third",
            "┊ fourth",
            "┊ fifth",
        ]
    );
    let collapsed =
        ThinkingBlockCell::new(message, 4, true, Some(Duration::from_secs(2))).display_lines(80);
    assert_eq!(
        collapsed
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "┊ Thinking (2.0s) — Alt+T to expand",
            "┊   text",
            "┊ first",
            "┊  ... 4 more lines",
        ]
    );
    assert!(
        ThinkingBlockCell::new("", 4, false, None)
            .display_lines(80)
            .is_empty()
    );
    assert!(
        ThinkingBlockCell::from_stream(&[], None)
            .display_lines(80)
            .is_empty()
    );
}

#[test]
fn short_streams_have_no_hidden_row_summary() {
    let source = [Line::from("only row")];
    assert_eq!(
        ThinkingBlockCell::from_stream(&source, None)
            .display_lines(80)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["┊ Thinking", "┊ only row"]
    );
}
