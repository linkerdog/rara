use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    widgets::{Paragraph, Widget},
};

use super::*;
use crate::tui::transcript_text::wrap_lines;

fn rendered(source: &str, width: usize) -> Vec<Line<'static>> {
    render_markdown_text_with_width_and_cwd(source, Some(width), Some(Path::new("/work"))).lines
}

fn plain(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn paint(lines: &[Line<'_>], width: u16) -> Buffer {
    let rows = wrap_lines(lines, width);
    let area = Rect::new(0, 0, width, rows.len().try_into().unwrap());
    let mut buffer = Buffer::empty(area);
    Paragraph::new(rows).render(area, &mut buffer);
    buffer
}

#[test]
fn task_markers_preserve_tight_loose_and_nested_list_structure() {
    for gap in ["\n", "\n\n"] {
        let source = format!("- [ ] outer{gap}  - [x] inner{gap}- [x] done\n");
        let lines = rendered(&source, 40);
        let nonempty = lines
            .iter()
            .map(Line::to_string)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            nonempty,
            ["\u{2610} outer", "    \u{2612} inner", "\u{2612} done"]
        );
        let buffer = paint(&lines, 40);
        for cell in &buffer.content {
            if matches!(cell.symbol(), "\u{2610}" | "\u{2612}") {
                assert!(cell.modifier.contains(Modifier::BOLD));
            }
        }
    }
}

#[test]
fn images_keep_alt_and_destinations_inside_links_and_cells() {
    let source = "[![**chart**](chart.png)](https://example.com/report)\n\n| Preview |\n| --- |\n| ![plot](plot.svg) |\n";
    let lines = rendered(source, 80);
    let text = plain(&lines);
    assert!(
        text.contains("chart (chart.png) (https://example.com/report)"),
        "{text}"
    );
    assert!(text.contains("plot (plot.svg)"), "{text}");
    assert!(lines.iter().flat_map(|line| &line.spans).any(|span| span.content == "chart" && span.style.add_modifier.contains(Modifier::BOLD)));
    assert!(plain(&rendered("![](empty.svg)", 20)).contains("(empty.svg)"));
}

#[test]
fn table_wraps_complete_cells_and_uses_short_column_space() {
    let source = "| K | Description |\n| - | - |\n| x | alpha beta gamma delta epsilon |\n";
    let lines = rendered(source, 24);
    let text = plain(&lines);
    assert!(text.contains("x | alpha beta gamma"), "{text}");
    for word in ["delta", "epsilon"] {
        assert!(text.contains(word), "missing {word}: {text}");
    }
    assert!(lines.iter().all(|line| line.width() <= 24));
    assert!(!text.contains('\u{2026}'));
}

#[test]
fn table_keeps_inline_styles_and_link_destinations_in_the_cell() {
    let source = "| V |\n| - |\n| **bold** *italic* `code` [docs](https://example.com) |\n";
    let lines = rendered(source, 80);
    let data = lines
        .iter()
        .find(|line| line.to_string().contains("bold"))
        .unwrap();
    assert!(data.to_string().contains("docs (https://example.com)"));
    for (word, style) in [
        ("bold", MarkdownStyles::new().strong),
        ("italic", MarkdownStyles::new().emphasis),
        ("code", MarkdownStyles::new().code),
        ("https://example.com", MarkdownStyles::new().link),
    ] {
        assert!(
            data.spans
                .iter()
                .any(|span| span.content.contains(word) && span.style == style),
            "missing style for {word}: {data:?}"
        );
    }
}

#[test]
fn narrow_tables_preserve_all_fields_without_overflow() {
    let source = "| A | B | C | D |\n| - | - | - | - |\n| one | two | three | four |\n";
    for width in [1, 2, 4, 8, 12] {
        let lines = rendered(source, width);
        assert!(
            lines.iter().all(|line| line.width() <= width),
            "width {width}: {}",
            plain(&lines)
        );
        let compact = plain(&lines)
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        for word in ["one", "two", "three", "four"] {
            assert!(compact.contains(word), "missing {word}: {compact}");
        }
    }
}

#[test]
fn empty_task_items_and_empty_images_stay_on_their_own_rows() {
    let source = "- [ ]\n- [x]\n- ![](empty.svg)\n- after\n";
    assert_eq!(
        rendered(source, 40)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>(),
        ["\u{2610} ", "\u{2612} ", "-  (empty.svg)", "- after"]
    );
}

#[test]
fn nested_tables_budget_for_indentation_without_leaking_cell_events() {
    for source in [
        "- | `Key` | Image |\n  | - | - |\n  | [label](/work/src/lib.rs) | ![alt](x.svg) |\n",
        "> | `Key` | Image |\n> | - | - |\n> | [label](/work/src/lib.rs) | ![alt](x.svg) |\n",
    ] {
        for width in [8, 16, 32, 80] {
            let lines = rendered(source, width);
            assert!(
                lines.iter().all(|line| line.width() <= width),
                "{width}: {}",
                plain(&lines)
            );
            let text = plain(&lines);
            assert!(!text.contains("label"), "{text}");
            assert!(
                !lines
                    .iter()
                    .any(|line| matches!(line.to_string().as_str(), "- " | "> ")),
                "orphaned marker: {text}"
            );
            let local_path = lines
                .iter()
                .flat_map(|line| &line.spans)
                .filter(|span| span.style.fg == MarkdownStyles::new().code.fg)
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert!(local_path.contains("src/lib.rs"), "{local_path}");
        }
    }
}

#[test]
fn table_buffers_preserve_styled_cjk_combining_and_joined_emoji() {
    let value = "\u{4e2d}\u{6587}e\u{301}\u{1f469}\u{1f3fd}\u{200d}\u{1f4bb}";
    let source = format!("| K | Value |\n| - | - |\n| x | **{value}** |\n");
    for width in [2, 5, 8, 12, 16, 24, 40, 80] {
        let lines = rendered(&source, width);
        assert!(
            lines.iter().all(|line| line.width() <= width),
            "{width}: {}",
            plain(&lines)
        );
        let buffer = paint(&lines, width.try_into().unwrap());
        let bold = buffer
            .content
            .iter()
            .filter(|cell| cell.modifier.contains(Modifier::BOLD))
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(bold, value, "width {width}");
    }
    let lines = rendered(&source, 1);
    assert!(lines.iter().all(|line| line.width() <= 1));
    assert!(plain(&lines).contains('\u{fffd}'));
}

#[test]
fn table_wrapping_preserves_inline_styles_through_actual_buffers() {
    let source = "| K | Value |\n| - | - |\n| x | **abcdefghijk** *lmnopqrstuv* `0123456789` [docs](https://example.com/long/path) |\n";
    for width in [8, 16, 24, 40, 80] {
        let lines = rendered(source, width);
        let buffer = paint(&lines, width.try_into().unwrap());
        for (modifier, expected) in [
            (Modifier::BOLD, "abcdefghijk"),
            (Modifier::ITALIC, "lmnopqrstuv"),
            (Modifier::UNDERLINED, "https://example.com/long/path"),
        ] {
            let actual = buffer
                .content
                .iter()
                .filter(|cell| cell.modifier.contains(modifier))
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert_eq!(actual, expected, "width {width}, {modifier:?}");
        }
        let code = buffer
            .content
            .iter()
            .filter(|cell| cell.fg == theme_color(ThemeToken::MarkdownCode))
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(code, "0123456789", "width {width}");
    }
}

#[test]
fn table_snapshots_cover_alignment_empty_cells_and_narrow_records() {
    let source = concat!(
        "| ID | Description | Count |\n",
        "| :-- | :--: | --: |\n",
        "| A | **alpha beta gamma delta** | 12 |\n",
        "| B | \u{4e2d}\u{6587} `value` | |\n",
    );
    let output = [8, 24, 48]
        .into_iter()
        .map(|width| {
            format!(
                "width {width}\n{}",
                super::tests::render_to_string_width(source, width)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    insta::assert_snapshot!("markdown_wrapped_tables", output);
}

#[test]
fn header_only_and_empty_headers_survive_vertical_fallback() {
    assert_eq!(
        plain(&rendered("| Alpha | Beta |\n| - | - |\n", 4)),
        "Alph\na\nBeta"
    );
    let lines = rendered("| | B |\n| - | - |\n| x | y |\n", 4);
    let compact = plain(&lines)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    assert_eq!(compact, "Column1:xB:y");
}

#[test]
fn sanitized_table_content_controls_width_and_preserves_visible_labels() {
    let lines = rendered("| K | V |\n| - | - |\n| x | **a\u{202e}b** |\n", 16);
    assert!(lines.iter().all(|line| line.width() <= 16));
    let bold = lines
        .iter()
        .flat_map(|line| &line.spans)
        .filter(|span| span.style.add_modifier.contains(Modifier::BOLD))
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(bold, "a\u{27e6}U+202E\u{27e7}b");
}

#[test]
fn final_streamed_markdown_matches_canonical_across_chunks_and_widths() {
    use crate::tui::markdown_stream::MarkdownStreamCollector;

    let sources = [
        "- [ ] one\n\n  - [x] two\n\n![**alt**][image]\n\n[image]: ./plot.svg\n",
        "Intro\n\n> | K | V |\n> | - | - |\n> | x | **long value** ![plot](plot.svg) |\n\nAfter\n",
    ];
    for source in sources {
        for width in [8, 24, 80] {
            for (split, _) in source.char_indices().chain([(source.len(), '\0')]) {
                let mut stream = MarkdownStreamCollector::new(Some(width), Path::new("/work"));
                stream.push_delta(&source[..split]);
                stream.lines();
                if !source.contains('|') {
                    assert_eq!(stream.lines(), rendered(&source[..split], width));
                }
                stream.push_delta(&source[split..]);
                stream.lines();
                if !source.contains('|') {
                    assert_eq!(stream.lines(), rendered(source, width));
                }
                stream.finalize();
                assert_eq!(
                    stream.lines(),
                    rendered(source, width),
                    "width {width}, split {split}"
                );
            }
        }
    }
}

#[test]
fn linked_images_keep_local_and_nested_link_destinations_separate() {
    for (source, expected) in [
        (
            "[![alt](plot.svg)](/work/report.md)",
            "alt (plot.svg) (report.md)",
        ),
        (
            "![alt [docs](https://docs.example)](plot.svg)",
            "alt docs (https://docs.example) (plot.svg)",
        ),
        (
            "![alt [label](/work/file.rs)](plot.svg)",
            "alt file.rs (plot.svg)",
        ),
        (
            "![outer ![inner](inner.svg)](outer.svg)",
            "outer inner (inner.svg) (outer.svg)",
        ),
    ] {
        assert_eq!(plain(&rendered(source, 120)), expected);
    }
}
