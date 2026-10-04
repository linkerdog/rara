//! Materialized styled rows shared by transcript rendering, counting, and selection.

use ratatui::{
    layout::Alignment,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

use super::text_wrap::{WrapMode, WrapOptions, display_width, grapheme_width, wrap_ranges};

pub(crate) fn wrap_line(line: &Line<'_>, width: u16) -> Vec<Line<'static>> {
    let line = super::display_sanitize::sanitize_display_line_segments(line);
    let text = line.to_string();
    let width = usize::from(width.max(1));
    let ranges = wrap_ranges(
        &text,
        WrapOptions {
            width,
            initial_indent: 0,
            subsequent_indent: 0,
            mode: WrapMode::Word,
        },
    );
    let mut span_index = 0;
    let mut span_end = line.spans.first().map_or(0, |span| span.content.len());
    ranges
        .into_iter()
        .map(|range| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (offset, grapheme) in text[range.clone()].grapheme_indices(true) {
                while range.start + offset >= span_end && span_index + 1 < line.spans.len() {
                    span_index += 1;
                    span_end += line.spans[span_index].content.len();
                }
                // A cluster crossing a style boundary stays indivisible and uses
                // the first contributing span's style, including its base character.
                let style = line
                    .spans
                    .get(span_index)
                    .map_or(line.style, |span| span.style);
                let content = if grapheme_width(grapheme) > width {
                    "\u{fffd}".to_string()
                } else {
                    grapheme.to_string()
                };
                if let Some(last) = spans.last_mut()
                    && last.style == style
                {
                    last.content.to_mut().push_str(&content);
                } else {
                    spans.push(Span::styled(content, style));
                }
            }
            let mut row = Line::from(spans).style(line.style);
            let row_width = display_width(&row.to_string());
            let padding = match line.alignment.unwrap_or(Alignment::Left) {
                Alignment::Left => 0,
                Alignment::Center => width.saturating_sub(row_width) / 2,
                Alignment::Right => width.saturating_sub(row_width),
            };
            if padding > 0 {
                row.spans.insert(0, Span::raw(" ".repeat(padding)));
            }
            row
        })
        .collect()
}

pub(crate) fn wrap_lines(lines: &[Line<'_>], width: u16) -> Vec<Line<'static>> {
    lines
        .iter()
        .flat_map(|line| wrap_line(line, width))
        .collect()
}
