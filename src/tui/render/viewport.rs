use ratatui::{layout::Rect, text::Line, widgets::Paragraph};

use crate::tui::custom_terminal::Frame;
use crate::tui::transcript_rows::TranscriptRows;

pub(crate) struct TranscriptViewport {
    /// Authoritative visual rows, already wrapped and ready to draw.
    pub(crate) lines: TranscriptRows,
    pub(crate) scroll_offset: usize,
    #[cfg(test)]
    pub(crate) work: crate::tui::transcript_work::WorkMeter,
}

impl TranscriptViewport {
    #[cfg(test)]
    pub(crate) fn new(lines: Vec<Line<'static>>, scroll_offset: usize, width: u16) -> Self {
        Self {
            lines: TranscriptRows::from_visual_lines(crate::tui::transcript_text::wrap_lines(
                &lines, width,
            )),
            scroll_offset,
            #[cfg(test)]
            work: crate::tui::transcript_work::WorkMeter::default(),
        }
    }

    /// Direct visual-row indexing also covers partial logical-line scrolling.
    pub(crate) fn visible_window(&self, height: u16) -> Vec<Line<'static>> {
        if self.lines.is_empty() || height == 0 {
            return Vec::new();
        }

        // Breathing room is handled by the caller (transcript_viewport)
        // when computing scroll_offset.
        let visible_rows = usize::from(height);
        let target_start = self.scroll_offset;
        let target_end = target_start.saturating_add(visible_rows);

        if target_start >= self.lines.len() {
            return Vec::new();
        }

        let lines: Vec<_> = (target_start..target_end.min(self.lines.len()))
            .filter_map(|row| self.lines.get(row).map(|row| row.line.clone()))
            .collect();
        #[cfg(test)]
        self.work
            .record(crate::tui::transcript_work::WorkKind::Clone, lines.len());
        lines
    }

    pub(crate) fn render(&self, f: &mut Frame, area: Rect) {
        let visible_lines = self.visible_window(area.height);
        f.render_widget(Paragraph::new(visible_lines), area);
    }
}

#[cfg(test)]
#[path = "viewport_tests.rs"]
mod tests;
