//! Styled physical-line clipping in terminal columns, after display normalization.

use ratatui::text::{Line, Span};

use super::display_sanitize::sanitize_display_line_segments;
use super::text_wrap::{display_width, truncate_to_width};

pub(crate) fn truncate_line_to_width(line: &Line<'_>, width: u16) -> Line<'static> {
    let mut line = sanitize_display_line_segments(line);
    let width = usize::from(width);
    if width == 0 {
        line.spans.clear();
        return line;
    }
    if display_width(&line.to_string()) <= width {
        return line;
    }

    // Preserve the existing ASCII diagnostic marker; very narrow rows retain
    // content instead of spending every column on the marker.
    let marker = if width > 3 { "..." } else { "" };
    let mut remaining = width - marker.len();
    let mut spans = Vec::new();
    for span in &line.spans {
        let prefix = truncate_to_width(&span.content, remaining);
        if !prefix.is_empty() {
            spans.push(Span::styled(prefix.to_owned(), span.style));
            remaining -= display_width(prefix);
        }
        if prefix.len() < span.content.len() {
            break;
        }
    }
    if !marker.is_empty() {
        let style = spans.last().map_or(line.style, |span| span.style);
        spans.push(Span::styled(marker, style));
    }
    line.spans = spans;
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_marker_and_zero_width_contract_are_preserved() {
        for (width, expected) in [(0, ""), (2, "ab"), (3, "abc"), (4, "a..."), (6, "abcdef")] {
            assert_eq!(
                truncate_line_to_width(&Line::from("abcdef"), width).to_string(),
                expected
            );
        }
    }

    #[test]
    fn clipping_preserves_styles_and_whole_graphemes() {
        let style = ratatui::style::Style::default().fg(ratatui::style::Color::Red);
        let line = Line::from(vec![
            Span::styled("\u{754c}a", style),
            Span::raw("\u{301}\u{1f469}\u{200d}\u{1f4bb}more"),
        ]);
        for width in 0..12 {
            let clipped = truncate_line_to_width(&line, width);
            assert!(display_width(&clipped.to_string()) <= usize::from(width));
            if width == 3 {
                assert_eq!(clipped.to_string(), "\u{754c}a\u{301}");
                assert_eq!(clipped.spans[0].style, style);
            }
        }
    }
}
