use ratatui::{layout::Rect, text::Line, widgets::Paragraph};

use crate::tui::custom_terminal::Frame;

pub(crate) struct TranscriptViewport {
    /// Authoritative visual rows, already wrapped and ready to draw.
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) scroll_offset: u16,
}

impl TranscriptViewport {
    pub(crate) fn new(lines: Vec<Line<'static>>, scroll_offset: u16, width: u16) -> Self {
        Self {
            lines: crate::tui::transcript_text::wrap_lines(&lines, width),
            scroll_offset,
        }
    }

    /// Direct visual-row indexing also covers partial logical-line scrolling.
    pub(crate) fn visible_window(&self, height: u16) -> &[Line<'static>] {
        if self.lines.is_empty() || height == 0 {
            return &[];
        }

        // Breathing room is handled by the caller (transcript_viewport)
        // when computing scroll_offset.
        let visible_rows = usize::from(height);
        let target_start = usize::from(self.scroll_offset);
        let target_end = target_start.saturating_add(visible_rows);

        if target_start >= self.lines.len() {
            return &[];
        }

        &self.lines[target_start..target_end.min(self.lines.len())]
    }

    pub(crate) fn render(&self, f: &mut Frame, area: Rect) {
        let visible_lines = self.visible_window(area.height);
        f.render_widget(Paragraph::new(visible_lines.to_vec()), area);
    }
}

#[cfg(test)]
#[path = "viewport_tests.rs"]
mod tests;
