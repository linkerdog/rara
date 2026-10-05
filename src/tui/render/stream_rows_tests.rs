use std::path::Path;

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use super::{ResponseView, StreamRowCache};
use crate::tui::{
    markdown_stream::{MarkdownStreamCollector, RenderedStream},
    text_wrap::display_width,
    transcript_rows::TranscriptRows,
    transcript_text::wrap_lines,
};

// Keep the pre-cache stream chrome independent of the production row helpers.
pub(super) fn canonical_response(
    lines: &[Line<'static>],
    width: u16,
    view: ResponseView,
) -> Vec<Line<'static>> {
    if lines.is_empty() {
        return wrap_lines(&[Line::from("• ")], width);
    }
    let cap = match view {
        ResponseView::Full => lines.len(),
        ResponseView::Compact => lines.len().min(4),
    };
    let mut logical = lines
        .iter()
        .take(cap)
        .enumerate()
        .map(|(index, line)| {
            let mut spans = vec![
                Span::raw(if index == 0 { "• " } else { "  " }),
                Span::raw("  "),
            ];
            spans.extend(line.spans.clone());
            Line::from(spans).style(line.style)
        })
        .collect::<Vec<_>>();
    if cap < lines.len() {
        logical.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("  ... {} more line(s)", lines.len() - cap),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    wrap_lines(&logical, width)
}

fn assert_rows(rows: &TranscriptRows, expected: &[Line<'static>]) {
    assert_eq!(rows.len(), expected.len());
    for (index, expected) in expected.iter().enumerate() {
        let actual = rows.get(index).expect("materialized visual row");
        assert_eq!(&actual.line, expected, "styled row {index}");
        assert_eq!(actual.text, expected.to_string());
        assert_eq!(actual.width, display_width(&actual.text));
    }
    assert!(rows.get(rows.len()).is_none());
    assert!(rows.get(usize::MAX).is_none());
}

fn materialize(
    collector: &mut MarkdownStreamCollector,
    layout: &mut StreamRowCache,
    width: u16,
    view: ResponseView,
) -> TranscriptRows {
    collector.lines();
    layout.materialize(collector.rendered_stream(), width, view)
}

fn check_collector(
    collector: &mut MarkdownStreamCollector,
    layout: &mut StreamRowCache,
    width: u16,
    view: ResponseView,
) {
    let expected = canonical_response(collector.lines(), width, view);
    assert_rows(&materialize(collector, layout, width, view), &expected);
}

#[test]
fn incremental_response_rows_match_canonical_chrome_for_structural_chunks() {
    let sources = [
        "# Heading\n\nPlain **bold** and \u{4e2d}\u{6587} 👩‍💻.\n\nNext paragraph.\n",
        "- first\n- second\n\n  continuation\n\nAfter the list.\n",
        "```rust\n/* comment\ncontinued */\nlet value = 42;\n```\n\nAfter code.\n",
        "~~~text\nraw\ttext\n~~~\n\nAfter code.\n",
        "> quoted\n> ```rust\n> let value = 1;\n> ```\n\nAfter quote.\n",
        "   ```rust\n   let value = 1;\n   ```\n\nAfter indented code.\n",
        "[first][id]\n\nUnrelated paragraph.\n\n[id]: https://example.com\n",
        "Introduction.\n\n| Header | Value |\n| --- | --- |\n| a | b |\n\nAfter table.\n",
        "```unknown-language\nfirst\r\nsecond\0\n```\n",
        "```rust\nfirst\n```not a closer\nlast\n```\n",
    ];
    for source in sources {
        let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
        let mut layout = StreamRowCache::default();
        for (index, character) in source.chars().enumerate() {
            collector.push_delta(character.encode_utf8(&mut [0; 4]));
            check_collector(&mut collector, &mut layout, 12, ResponseView::Full);
            if index % 7 == 0 {
                check_collector(&mut collector, &mut layout, 8, ResponseView::Compact);
                check_collector(&mut collector, &mut layout, 12, ResponseView::Full);
            }
        }
        collector.finalize();
        let full_source = crate::tui::markdown_render::render_markdown_text_with_width_and_cwd(
            source,
            None,
            Some(Path::new("/workspace")),
        )
        .lines;
        for width in [1, 2, 8, 80, 120, 160] {
            for view in [ResponseView::Full, ResponseView::Compact] {
                check_collector(&mut collector, &mut layout, width, view);
                assert_rows(
                    &materialize(&mut collector, &mut layout, width, view),
                    &canonical_response(&full_source, width, view),
                );
            }
        }
    }
}

#[test]
fn live_response_rows_match_full_source_rendering_at_every_character() {
    let sources = [
        "# Heading\n\nPlain **bold** and \u{4e2d}\u{6587} 👩‍💻.\n\nNext paragraph.\n",
        "- first\n- second\n\n  continuation\n\nAfter the list.\n",
        "```rust\n/* comment\ncontinued */\nlet value = 42;\n```\n\nAfter code.\n",
        "> quoted\n> ```rust\n> let value = 1;\n> ```\n\nAfter quote.\n",
        "[first][id]\n\nUnrelated paragraph.\n\n[id]: https://example.com\n",
        "[id]: first\n\nSee [id].\n\nNext [late].\n\n[late]: second\n\nAfter [id] [late].\n",
        "[id]: first\n\nSee [id].\n\n[ID]: other\n\nAfter [ID].\n",
    ];
    for source in sources {
        let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
        let mut layout = StreamRowCache::default();
        for (offset, ch) in source.char_indices() {
            let end = offset + ch.len_utf8();
            collector.push_delta(&source[offset..end]);
            let full_source = crate::tui::markdown_render::render_markdown_text_with_width_and_cwd(
                &source[..end],
                None,
                Some(Path::new("/workspace")),
            )
            .lines;
            for width in [12, 80] {
                for view in [ResponseView::Full, ResponseView::Compact] {
                    assert_rows(
                        &materialize(&mut collector, &mut layout, width, view),
                        &canonical_response(&full_source, width, view),
                    );
                }
            }
        }
    }
}

#[test]
fn unchanged_response_reads_retain_rows_without_layout_work() {
    let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
    let mut layout = StreamRowCache::default();
    collector.push_delta(&"Stable paragraph.\n\n".repeat(1000));
    let original = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
    let before = layout.work.get();
    for _ in 0..100 {
        let rows = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
        assert_eq!(layout.work.get(), before);
        assert!(std::ptr::eq(original.get(0).unwrap(), rows.get(0).unwrap()));
        assert!(std::ptr::eq(
            original.get(original.len() - 1).unwrap(),
            rows.get(rows.len() - 1).unwrap()
        ));
    }
}

#[test]
fn response_replacement_invalidates_same_length_source_and_retains_old_snapshot() {
    let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
    let mut layout = StreamRowCache::default();
    let old = "first\n\nold\n\nlast\n";
    let new = "first\n\nnew\n\nlast\n";
    collector.push_delta(old);
    let retained = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
    collector.replace_source(new);
    check_collector(&mut collector, &mut layout, 80, ResponseView::Full);
    let replaced = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
    assert_eq!(retained.len(), replaced.len());
    assert!(retained.iter().any(|line| line.to_string().contains("old")));
    assert!(replaced.iter().any(|line| line.to_string().contains("new")));
    assert!(!replaced.iter().any(|line| line.to_string().contains("old")));
}

#[test]
fn finalization_reveals_held_rows_without_appending_source() {
    let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
    let mut layout = StreamRowCache::default();
    collector.push_delta(
        "Introduction.\n\n| Header | Value |\n| --- | --- |\n| a | b |\n\nAfter table.\n",
    );
    let retained = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
    assert!(
        !retained
            .iter()
            .any(|line| line.to_string().contains("Header"))
    );
    collector.finalize();
    check_collector(&mut collector, &mut layout, 80, ResponseView::Full);
    let finalized = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
    assert!(
        finalized
            .iter()
            .any(|line| line.to_string().contains("Header"))
    );
    assert!(
        finalized
            .iter()
            .any(|line| line.to_string().contains("After table."))
    );
    assert!(retained.len() < finalized.len());
    assert!(
        !retained
            .iter()
            .any(|line| line.to_string().contains("Header"))
    );
}

#[test]
fn fence_closer_and_normalization_replay_previously_retained_code() {
    for suffix in ["```\nAfter code.\n", "new\r\n", "new\0\n"] {
        let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
        let mut layout = StreamRowCache::default();
        collector.push_delta("```rust\nlet first = 1;\n");
        let retained = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
        collector.push_delta(suffix);
        check_collector(&mut collector, &mut layout, 80, ResponseView::Full);
        assert!(
            retained
                .iter()
                .any(|line| line.to_string().contains("let first"))
        );
        collector.finalize();
        check_collector(&mut collector, &mut layout, 80, ResponseView::Full);
    }
}

#[test]
fn fence_highlight_limit_replay_does_not_retain_stale_styled_prefix() {
    let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
    let mut layout = StreamRowCache::default();
    collector.push_delta("```rust\nlet first = 1;\n");
    let retained = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
    let code_row = retained
        .iter()
        .position(|line| line.to_string().contains("let first"))
        .unwrap();
    collector.push_delta(&"let next = 2;\n".repeat(10_001));
    check_collector(&mut collector, &mut layout, 80, ResponseView::Full);
    let replayed = materialize(&mut collector, &mut layout, 80, ResponseView::Full);
    assert_ne!(
        retained.get(code_row).unwrap().line,
        replayed.get(code_row).unwrap().line
    );
}

#[test]
fn empty_width_and_compact_view_transitions_match_frozen_stream_chrome() {
    let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
    let mut layout = StreamRowCache::default();
    for source in [
        "",
        "\n",
        "first\n\n",
        "second\n\nthird\n\nfourth\n\nfifth\n",
    ] {
        collector.push_delta(source);
        for width in [0, 1, 2, 8, 80, 120, 160] {
            for view in [
                ResponseView::Full,
                ResponseView::Compact,
                ResponseView::Full,
            ] {
                check_collector(&mut collector, &mut layout, width, view);
            }
        }
    }
}

#[test]
fn explicit_epoch_refreshes_same_revision_style_and_alignment() {
    let mut cache = StreamRowCache::default();
    let original = vec![Line::from(Span::styled(
        "same",
        Style::default().fg(Color::Red),
    ))];
    let retained = cache.materialize(
        RenderedStream {
            epoch: 0,
            revision: 4,
            stable_lines: 1,
            plain_start: None,
            lines: &original,
        },
        80,
        ResponseView::Full,
    );
    let changed =
        vec![Line::from(Span::styled("same", Style::default().fg(Color::Blue))).right_aligned()];
    let rows = cache.materialize(
        RenderedStream {
            epoch: 1,
            revision: 4,
            stable_lines: 1,
            plain_start: None,
            lines: &changed,
        },
        80,
        ResponseView::Full,
    );
    assert_rows(&rows, &canonical_response(&changed, 80, ResponseView::Full));
    assert_ne!(retained.get(0).unwrap().line, rows.get(0).unwrap().line);
    assert!(rows.get(0).unwrap().line.alignment.is_none());
}

#[test]
fn compact_growth_only_wraps_new_head_and_summary_rows() {
    let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
    let mut layout = StreamRowCache::default();
    for index in 0..200 {
        collector.push_delta(&format!("Paragraph {index}.\n\n"));
        check_collector(&mut collector, &mut layout, 80, ResponseView::Compact);
    }
    let work = layout.work.get();
    assert!(work.wrapped_lines <= 200 * 4, "{work:?}");
    assert!(work.cloned_rows <= 200 * 4, "{work:?}");
    assert_eq!(work.hashed_rows, 0);
}

#[test]
fn plain_soft_break_rows_reuse_completed_lines_and_replay_late_heading() {
    for width in [1, 8, 80] {
        for view in [ResponseView::Full, ResponseView::Compact] {
            let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
            let mut layout = StreamRowCache::default();
            let mut source = "Ordinary paragraph line\n".to_string();
            collector.push_delta(&source);
            let original = materialize(&mut collector, &mut layout, width, view);
            for _ in 0..200 {
                let chunk = "Another paragraph line\n";
                source.push_str(chunk);
                collector.push_delta(chunk);
                let rows = materialize(&mut collector, &mut layout, width, view);
                assert!(std::ptr::eq(original.get(0).unwrap(), rows.get(0).unwrap()));
            }
            let work = layout.work.get();
            assert!(work.cloned_rows <= 201, "{work:?}");
            assert!(work.wrapped_lines <= 402, "{work:?}");
            check_collector(&mut collector, &mut layout, width, view);

            source.push_str("===\n");
            collector.push_delta("===\n");
            let full = crate::tui::markdown_render::render_markdown_text_with_width_and_cwd(
                &source,
                None,
                Some(Path::new("/workspace")),
            )
            .lines;
            let replayed = materialize(&mut collector, &mut layout, width, view);
            assert_rows(&replayed, &canonical_response(&full, width, view));
            assert!(!std::ptr::eq(
                original.get(0).unwrap(),
                replayed.get(0).unwrap()
            ));
            assert_rows(
                &original,
                &canonical_response(&[Line::from("Ordinary paragraph line")], width, view),
            );
        }
    }
}

#[test]
#[expect(
    clippy::print_stderr,
    reason = "Expose byte counts that row-only metrics hide for long physical lines."
)]
fn growing_physical_line_layout_work_is_linear() {
    let mut measurements = Vec::new();
    for width in [8, 80] {
        for chunk in ["ordinary words ", "unbrokenword", "Words 👩‍💻 cafe\u{301} "] {
            let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
            let mut layout = StreamRowCache::default();
            let mut source = String::new();
            for _ in 0..200 {
                source.push_str(chunk);
                collector.push_delta(chunk);
                materialize(&mut collector, &mut layout, width, ResponseView::Full);
            }
            let expected = crate::tui::markdown_render::render_markdown_text_with_width_and_cwd(
                &source,
                None,
                Some(Path::new("/workspace")),
            )
            .lines;
            assert_rows(
                &materialize(&mut collector, &mut layout, width, ResponseView::Full),
                &canonical_response(&expected, width, ResponseView::Full),
            );
            let work = layout.work.get();
            eprintln!(
                "{width} columns, {chunk:?}, {} source bytes: {work:?}",
                source.len()
            );
            measurements.push((width, chunk, source.len(), work));
        }
    }
    for (width, chunk, bytes, work) in measurements {
        // Three mutable visual rows plus new UTF-8 content; width is part of the
        // layout bound, independently of the total accumulated source length.
        let budget = bytes * 4 + 200 * usize::from(width) * 16;
        assert!(
            work.cloned_bytes <= budget,
            "{width} columns, {chunk:?}: {work:?}"
        );
        assert!(
            work.wrapped_bytes <= budget,
            "{width} columns, {chunk:?}: {work:?}"
        );
    }
}

#[test]
fn growing_line_matches_canonical_through_graphemes_projection_and_completion() {
    let prefix = "Ordinary words with spaces. ".repeat(8);
    let sources = [
        format!("{prefix}{} tail", "unbrokenword".repeat(20)),
        format!("{prefix}👩‍💻 cafe\u{301} \u{4e2d}\u{6587} 🇬🇧🇺🇸 end"),
        format!("{prefix}❤\u{fe0f} and 👩‍❤️‍💋‍👩 with a\u{ff9e}\u{ff9e}\u{ff9e} tail"),
        format!("{prefix}{}\u{301} tail", "a".repeat(100)),
        format!("{prefix}\u{200b}More words\nNext ordinary line grows again."),
        format!("{prefix}\u{202e}More words\nNext ordinary line grows again."),
        format!("{prefix}\n{}\nLast", "New physical line words. ".repeat(8)),
        format!("{prefix}\n===\nAfter heading"),
        format!("# Heading\n\n{prefix}*late emphasis* and [link](destination)"),
        format!("First\nSecond\nThird\n{prefix}\nFifth\nSixth"),
    ];
    for source in sources {
        for width in [1, 8, 80] {
            for view in [ResponseView::Full, ResponseView::Compact] {
                let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
                let mut layout = StreamRowCache::default();
                for (offset, ch) in source.char_indices() {
                    let end = offset + ch.len_utf8();
                    collector.push_delta(&source[offset..end]);
                    let full =
                        crate::tui::markdown_render::render_markdown_text_with_width_and_cwd(
                            &source[..end],
                            None,
                            Some(Path::new("/workspace")),
                        )
                        .lines;
                    assert_rows(
                        &materialize(&mut collector, &mut layout, width, view),
                        &canonical_response(&full, width, view),
                    );
                }
            }
        }
    }
}

#[test]
fn growing_line_retains_prefix_snapshots_and_resets_on_layout_or_source_changes() {
    let mut collector = MarkdownStreamCollector::new(None, Path::new("/workspace"));
    let mut layout = StreamRowCache::default();
    let mut source = "Ordinary words ".repeat(100);
    collector.push_delta(&source);
    let original = materialize(&mut collector, &mut layout, 8, ResponseView::Full);
    let frozen = original.iter().cloned().collect::<Vec<_>>();
    for chunk in ["more words", " more", "\n", "Next physical line"] {
        source.push_str(chunk);
        collector.push_delta(chunk);
        let rows = materialize(&mut collector, &mut layout, 8, ResponseView::Full);
        assert!(std::ptr::eq(original.get(0).unwrap(), rows.get(0).unwrap()));
        check_collector(&mut collector, &mut layout, 8, ResponseView::Full);
    }
    assert_rows(&original, &frozen);
    for width in [0, 1, 80, 8] {
        for view in [ResponseView::Compact, ResponseView::Full] {
            check_collector(&mut collector, &mut layout, width, view);
        }
    }
    let replacement = source.replace("Ordinary", "Replaced");
    assert_eq!(source.len(), replacement.len());
    collector.replace_source(&replacement);
    let replaced = materialize(&mut collector, &mut layout, 8, ResponseView::Full);
    assert!(!std::ptr::eq(
        original.get(0).unwrap(),
        replaced.get(0).unwrap()
    ));
    check_collector(&mut collector, &mut layout, 8, ResponseView::Full);
    assert_rows(&original, &frozen);
}
