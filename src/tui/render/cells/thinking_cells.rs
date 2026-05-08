use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::HistoryCell;
use super::responding_cell::markdown_body_lines;
use crate::tui::markdown_render::render_markdown_text_with_width;
use crate::tui::render::section_label;
use crate::tui::theme::*;

pub(crate) struct ThinkingTextCell {
    message: String,
    max_lines: usize,
}

impl ThinkingTextCell {
    pub(crate) fn new(message: &str, max_lines: usize) -> Self {
        Self {
            message: message.to_string(),
            max_lines,
        }
    }
}

impl HistoryCell for ThinkingTextCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let render_width = usize::from(width.saturating_sub(2));
        let rendered = render_markdown_text_with_width(&self.message, Some(render_width));
        let rendered_lines = rendered.lines;
        let start = rendered_lines.len().saturating_sub(self.max_lines);
        let body = markdown_body_lines(&rendered_lines[start..], self.max_lines);
        let mut lines = vec![Line::from(section_label("Thinking", PHASE_THINKING))];
        if start > 0 {
            lines.push(Line::from(Span::styled(
                format!("  ... {start} more line(s)"),
                Style::default().fg(TEXT_SECONDARY),
            )));
        }
        lines.extend(body.into_iter().map(|mut line| {
            line.spans.insert(0, Span::raw("  "));
            line
        }));
        lines
    }
}

pub(crate) struct ThinkingGroupCell<'a> {
    message: String,
    stream_lines: Option<&'a [Line<'static>]>,
    max_lines: usize,
}

impl<'a> ThinkingGroupCell<'a> {
    pub(crate) fn new(
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

impl HistoryCell for ThinkingGroupCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let render_width = usize::from(width.saturating_sub(2));
        let mut rendered_lines = Vec::new();
        if !self.message.is_empty() {
            let rendered = render_markdown_text_with_width(&self.message, Some(render_width));
            rendered_lines.extend(rendered.lines);
        }
        if let Some(stream_lines) = self.stream_lines {
            rendered_lines.extend(stream_lines.iter().cloned());
        }

        let start = rendered_lines.len().saturating_sub(self.max_lines);
        let body = markdown_body_lines(&rendered_lines[start..], self.max_lines);
        let mut lines = vec![Line::from(section_label("Thinking", PHASE_THINKING))];
        if start > 0 {
            lines.push(Line::from(Span::styled(
                format!("  ... {start} more line(s)"),
                Style::default().fg(TEXT_SECONDARY),
            )));
        }
        lines.extend(body.into_iter().map(|mut line| {
            line.spans.insert(0, Span::raw("  "));
            line
        }));
        lines
    }
}
