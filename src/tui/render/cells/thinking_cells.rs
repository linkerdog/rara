use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::HistoryCell;
use crate::tui::markdown_render::render_markdown_text_with_width;
use crate::tui::theme::TEXT_MUTED;

/// Renders thinking content as dimmed lines with a ┊ accent prefix.
///
/// When collapsed (default): shows first 2 lines plus duration summary.
/// When expanded (Alt+T): shows last max_lines lines (stale-tail for stream).
/// The `duration` field is Some for committed turns with a known duration.
pub(crate) struct ThinkingBlockCell<'a> {
    message: String,
    stream_lines: Option<&'a [Line<'static>]>,
    max_lines: usize,
    collapsed: bool,
    duration: Option<std::time::Duration>,
}

impl<'a> ThinkingBlockCell<'a> {
    pub(crate) fn new(
        message: &str,
        max_lines: usize,
        collapsed: bool,
        duration: Option<std::time::Duration>,
    ) -> Self {
        Self {
            message: message.to_string(),
            stream_lines: None,
            max_lines,
            collapsed,
            duration,
        }
    }

    pub(crate) fn with_stream_lines(
        message: String,
        stream_lines: Option<&'a [Line<'static>]>,
        max_lines: usize,
        collapsed: bool,
        duration: Option<std::time::Duration>,
    ) -> Self {
        Self {
            message,
            stream_lines,
            max_lines,
            collapsed,
            duration,
        }
    }

    fn duration_label(&self) -> Option<String> {
        self.duration.map(|d| format!(" ({:.1}s)", d.as_secs_f64()))
    }

    fn collapse_hint(&self) -> Option<String> {
        if self.duration.is_some() {
            let action = if self.collapsed { "expand" } else { "collapse" };
            Some(format!(" — Alt+T to {action}"))
        } else {
            None
        }
    }
}

impl HistoryCell for ThinkingBlockCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let render_width = usize::from(width.saturating_sub(2));
        let mut rendered_lines: Vec<Line<'static>> = Vec::new();

        if !self.message.is_empty() {
            let rendered = render_markdown_text_with_width(&self.message, Some(render_width));
            rendered_lines.extend(rendered.lines);
        }
        if let Some(lines) = self.stream_lines {
            rendered_lines.extend(lines.iter().cloned());
        }

        if rendered_lines.is_empty() {
            return vec![];
        }

        // Compute how many content lines to show.
        let effective_max = if self.collapsed {
            2usize.min(self.max_lines)
        } else {
            self.max_lines
        };

        // Build heading line with duration + collapse hint.
        let mut heading_parts: Vec<String> = vec!["Thinking".to_string()];
        if let Some(dur) = self.duration_label() {
            heading_parts.push(dur);
        }
        if let Some(hint) = self.collapse_hint() {
            heading_parts.push(hint);
        }

        let total = rendered_lines.len();
        let mut lines = Vec::with_capacity(effective_max + 2);

        lines.push(Line::from(Span::styled(
            format!("┊ {}", heading_parts.join("")),
            Style::default().fg(TEXT_MUTED),
        )));

        if self.collapsed {
            // Show first N lines (preview mode).
            let head = &rendered_lines[..effective_max.min(total)];
            for line in head {
                let mut accented = Line::from(Span::styled("┊ ", Style::default().fg(TEXT_MUTED)));
                for span in &line.spans {
                    accented.push_span(Span::styled(
                        span.content.to_string(),
                        span.style.patch(Style::default().fg(TEXT_MUTED)),
                    ));
                }
                lines.push(accented);
            }
            if total > effective_max {
                lines.push(Line::from(Span::styled(
                    format!("┊  ... {} more lines", total - effective_max),
                    Style::default().fg(TEXT_MUTED),
                )));
            }
        } else {
            // Show tail lines (stale-tail for stream, or full for expanded).
            let start = total.saturating_sub(effective_max);
            if start > 0 {
                lines.push(Line::from(Span::styled(
                    format!("┊  ... {start} more lines"),
                    Style::default().fg(TEXT_MUTED),
                )));
            }
            let tail = &rendered_lines[start..];
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
        }
        lines
    }
}
