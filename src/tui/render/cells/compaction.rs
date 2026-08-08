use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::HistoryCell;
use crate::tui::state::CompactionTranscriptPayload;
use crate::tui::status_display::format_token_count;
use crate::tui::theme::{STATUS_INFO, TEXT_MUTED, TEXT_SECONDARY};

pub(crate) struct CompactionCell<'a> {
    payload: &'a CompactionTranscriptPayload,
    summary: &'a str,
}

impl<'a> CompactionCell<'a> {
    pub(crate) fn new(payload: &'a CompactionTranscriptPayload, summary: &'a str) -> Self {
        Self { payload, summary }
    }
}

impl HistoryCell for CompactionCell<'_> {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let saved = self
            .payload
            .before_tokens
            .saturating_sub(self.payload.after_tokens);
        let mut lines = vec![Line::from(vec![
            Span::styled(
                format!("Compaction #{}", self.payload.count),
                Style::default()
                    .fg(STATUS_INFO)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {} -> {} tokens  (saved {})",
                    format_token_count(self.payload.before_tokens),
                    format_token_count(self.payload.after_tokens),
                    format_token_count(saved),
                ),
                Style::default().fg(TEXT_SECONDARY),
            ),
        ])];

        if !self.summary.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  {}", self.summary.trim()),
                Style::default().fg(TEXT_MUTED),
            )));
        }
        if !self.payload.recent_files.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  recent files: {}", self.payload.recent_files.len()),
                Style::default().fg(TEXT_MUTED),
            )));
        }
        lines
    }
}
