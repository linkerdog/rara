use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::HistoryCell;
use crate::tui::markdown_render::render_markdown_text_with_width;
use crate::tui::theme::TEXT_MUTED;

/// Renders thinking content as dimmed lines with a ┊ accent prefix.
///
/// Committed blocks select a two-line head or `max_lines` tail; live streams
/// always select a four-line tail. Duration may be live elapsed time or a
/// recorded duration for a finalized block.
pub(crate) struct ThinkingBlockCell<'a> {
    content: ThinkingContent<'a>,
    max_lines: usize,
    collapsed: bool,
    duration: Option<std::time::Duration>,
    #[cfg(test)]
    work: Option<crate::tui::transcript_work::WorkMeter>,
}

enum ThinkingContent<'a> {
    Message(String),
    Stream(&'a [Line<'static>]),
}

impl<'a> ThinkingBlockCell<'a> {
    pub(crate) fn new(
        message: &str,
        max_lines: usize,
        collapsed: bool,
        duration: Option<std::time::Duration>,
    ) -> Self {
        Self {
            content: ThinkingContent::Message(message.to_string()),
            max_lines,
            collapsed,
            duration,
            #[cfg(test)]
            work: None,
        }
    }

    pub(crate) fn from_stream(
        lines: &'a [Line<'static>],
        duration: Option<std::time::Duration>,
    ) -> Self {
        Self {
            content: ThinkingContent::Stream(lines),
            max_lines: 4,
            collapsed: false,
            duration,
            #[cfg(test)]
            work: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_work_meter(mut self, work: crate::tui::transcript_work::WorkMeter) -> Self {
        self.work = Some(work);
        self
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
        let rendered;
        let rendered_lines = match &self.content {
            ThinkingContent::Message(message) => {
                rendered = render_markdown_text_with_width(message, Some(render_width));
                rendered.lines.as_slice()
            }
            ThinkingContent::Stream(lines) => lines,
        };

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
        let mut lines = Vec::with_capacity(effective_max.min(total) + 2);

        lines.push(Line::from(Span::styled(
            format!("┊ {}", heading_parts.join("")),
            Style::default().fg(TEXT_MUTED),
        )));

        // Select before copying: live thinking may retain thousands of hidden rows.
        let visible = if self.collapsed {
            &rendered_lines[..effective_max.min(total)]
        } else {
            &rendered_lines[total.saturating_sub(effective_max)..]
        };
        let summary = (visible.len() < total).then(|| {
            Line::from(Span::styled(
                format!("┊  ... {} more lines", total - visible.len()),
                Style::default().fg(TEXT_MUTED),
            ))
        });
        for line in visible {
            #[cfg(test)]
            if let Some(work) = &self.work {
                work.record(crate::tui::transcript_work::WorkKind::Clone, 1);
            }
            let mut accented = Line::from(Span::styled("┊ ", Style::default().fg(TEXT_MUTED)));
            for span in &line.spans {
                accented.push_span(Span::styled(
                    span.content.to_string(),
                    span.style.patch(Style::default().fg(TEXT_MUTED)),
                ));
            }
            lines.push(accented);
        }
        if let Some(summary) = summary {
            let index = if self.collapsed { lines.len() } else { 1 };
            lines.insert(index, summary);
        }
        lines
    }
}

#[cfg(test)]
#[path = "thinking_cells_tests.rs"]
mod tests;
