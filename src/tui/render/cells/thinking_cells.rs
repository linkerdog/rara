use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::HistoryCell;
use super::responding_cell::markdown_body_lines;
use crate::tui::markdown_render::render_markdown_text_with_width;
use crate::tui::theme::{TEXT_MUTED, TEXT_SECONDARY};

/// Renders a thinking block with collapsible display.
///
/// Streaming (live): ▾ Thinking · N lines → content with ┊ accent bars
/// Committed:          ▸ Thinking · N lines (≈T tokens) → summary only
///
/// Design blends Claude Code's collapsed-summary pattern and OpenCode's
/// ┊ border-left accent for expanded content.
///
/// NOTE: committed turns are always collapsed in this version. An `expanded`
/// field independent of `is_streaming` would enable Enter-key toggle (follow-up).
pub(crate) struct ThinkingBlockCell<'a> {
    message: String,
    stream_lines: Option<&'a [Line<'static>]>,
    max_lines: usize,
    is_streaming: bool,
}

impl<'a> ThinkingBlockCell<'a> {
    pub(crate) fn new(message: &str, max_lines: usize) -> Self {
        Self {
            message: message.to_string(),
            stream_lines: None,
            max_lines,
            is_streaming: false,
        }
    }

    pub(crate) fn with_stream_lines(
        message: String,
        stream_lines: Option<&'a [Line<'static>]>,
        max_lines: usize,
    ) -> Self {
        Self {
            message,
            stream_lines,
            max_lines,
            is_streaming: true,
        }
    }
}

impl HistoryCell for ThinkingBlockCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        // Raw line count (from newlines + stream lines) for collapsed summary.
        // Avoids expensive markdown rendering in the committed path.
        let raw_line_count =
            self.message.lines().count() + self.stream_lines.map_or(0, |l| l.len());

        if self.is_streaming {
            // --- Expanded: render markdown, show with ┊ accent bars ---
            let render_width = usize::from(width.saturating_sub(2));
            let mut rendered_lines: Vec<Line<'static>> = Vec::new();
            if !self.message.is_empty() {
                let rendered = render_markdown_text_with_width(&self.message, Some(render_width));
                rendered_lines.extend(rendered.lines.into_iter());
            }
            if let Some(lines) = self.stream_lines {
                rendered_lines.extend(lines.iter().cloned());
            }
            let total_line_count = rendered_lines.len();

            let header = Line::from(Span::styled(
                format!("▾ Thinking · {total_line_count} line(s)"),
                Style::default().fg(TEXT_SECONDARY),
            ));
            let start = total_line_count.saturating_sub(self.max_lines);
            let body = markdown_body_lines(&rendered_lines[start..], self.max_lines);
            let mut lines = vec![header];
            if start > 0 {
                lines.push(Line::from(Span::styled(
                    format!("┊  ... {start} more line(s)"),
                    Style::default().fg(TEXT_MUTED),
                )));
            }
            for line in body {
                let mut accented = Line::from(Span::styled("┊ ", Style::default().fg(TEXT_MUTED)));
                for span in line.spans {
                    accented.push_span(Span::styled(
                        span.content.to_string(),
                        span.style.fg(TEXT_MUTED),
                    ));
                }
                lines.push(accented);
            }
            lines
        } else {
            // --- Collapsed: summary only, no markdown render ---
            let token_estimate = thinking_token_estimate(&self.message, self.stream_lines);
            vec![Line::from(Span::styled(
                format!("▸ Thinking · {raw_line_count} line(s) (≈{token_estimate} tokens)"),
                Style::default().fg(TEXT_SECONDARY),
            ))]
        }
    }
}

/// Rough token estimate from thinking text and stream lines.
fn thinking_token_estimate(message: &str, stream_lines: Option<&[Line<'_>]>) -> usize {
    let mut chars: usize = message.chars().count();
    if let Some(lines) = stream_lines {
        for line in lines {
            chars += line.width();
        }
    }
    // Rough heuristic: ~4 chars per token for English text
    chars / 4
}
