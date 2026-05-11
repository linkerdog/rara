use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::HistoryCell;
use crate::tui::markdown_render::render_markdown_text_with_width;
use crate::tui::theme::TEXT_MUTED;

/// Renders thinking content as dimmed lines with `┊` accent prefix.
///
/// OpenCode-style: no header label, no toggle, no token count.
/// Content flows inline with muted color and left accent bar,
/// visually recessed as "internal monologue."
pub(crate) struct ThinkingBlockCell<'a> {
    message: String,
    stream_lines: Option<&'a [Line<'static>]>,
}

impl<'a> ThinkingBlockCell<'a> {
    pub(crate) fn new(message: &str, max_lines: usize) -> Self {
        let _ = max_lines;
        Self {
            message: message.to_string(),
            stream_lines: None,
        }
    }

    pub(crate) fn with_stream_lines(
        message: String,
        stream_lines: Option<&'a [Line<'static>]>,
        max_lines: usize,
    ) -> Self {
        let _ = max_lines;
        Self {
            message,
            stream_lines,
        }
    }
}

impl HistoryCell for ThinkingBlockCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let render_width = usize::from(width.saturating_sub(2));
        let mut rendered: Vec<Line<'static>> = Vec::new();
        if !self.message.is_empty() {
            let md = render_markdown_text_with_width(&self.message, Some(render_width));
            rendered.extend(md.lines.into_iter());
        }
        if let Some(lines) = self.stream_lines {
            rendered.extend(lines.iter().cloned());
        }
        let mut lines = Vec::with_capacity(rendered.len());
        for line in rendered {
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
    }
}
