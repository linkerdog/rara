use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::HistoryCell;
use crate::tui::markdown_render::render_markdown_text_with_width;
use crate::tui::theme::TEXT_MUTED;

/// Renders thinking content as dimmed lines with a ┊ accent prefix.
///
/// Design follows OpenCode: no header label, no toggle, no token count.
/// Every line is prefixed with ┊ and colored TEXT_MUTED for a subdued
/// "inner monologue" visual layer between # You and # Agent sections.
pub(crate) struct ThinkingBlockCell<'a> {
    message: String,
    stream_lines: Option<&'a [Line<'static>]>,
    max_lines: usize,
}

impl<'a> ThinkingBlockCell<'a> {
    pub(crate) fn new(message: &str, max_lines: usize) -> Self {
        Self {
            message: message.to_string(),
            stream_lines: None,
            max_lines,
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
        }
    }
}

impl HistoryCell for ThinkingBlockCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let render_width = usize::from(width.saturating_sub(2));
        let mut rendered_lines: Vec<Line<'static>> = Vec::new();

        if !self.message.is_empty() {
            let rendered = render_markdown_text_with_width(&self.message, Some(render_width));
            rendered_lines.extend(rendered.lines.into_iter());
        }
        if let Some(lines) = self.stream_lines {
            rendered_lines.extend(lines.iter().cloned());
        }

        let start = rendered_lines.len().saturating_sub(self.max_lines);
        let tail = &rendered_lines[start..];

        let mut lines = Vec::with_capacity(tail.len());
        if start > 0 {
            lines.push(Line::from(Span::styled(
                format!("┊  ... {start} more lines"),
                Style::default().fg(TEXT_MUTED),
            )));
        }
        for line in tail {
            let mut accented = Line::from(Span::styled("┊ ", Style::default().fg(TEXT_MUTED)));
            for span in &line.spans {
                accented.push_span(Span::styled(
                    span.content.to_string(),
                    span.style.patch(Style::default().fg(TEXT_MUTED)),
                ));
            }
            lines.push(accented);
        }
        lines
    }
}
