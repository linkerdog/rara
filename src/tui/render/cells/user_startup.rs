use std::path::Path;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::HistoryCell;
use crate::tui::render::{
    display_width, prefixed_message_lines, startup_card_inner_width, truncate_for_startup_card,
    truncate_path_middle,
};
use crate::tui::theme::*;

// ── UserCell ──

pub(crate) struct UserCell {
    message: String,
}

impl UserCell {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl HistoryCell for UserCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        prefixed_message_lines("You", &self.message, 4)
    }
}

// ── StartupCardCell ──

pub(crate) struct StartupCardCell {
    model_label: String,
    directory: String,
}

impl StartupCardCell {
    pub(crate) fn new(model_label: String, directory: String) -> Self {
        Self {
            model_label,
            directory,
        }
    }
}

impl HistoryCell for StartupCardCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let Some(inner_width) = startup_card_inner_width(width) else {
            return Vec::new();
        };

        let model_prefix = "model · ";
        let dir_prefix = "dir   · ";
        let hint = "/model to change";
        let hint_width = display_width(hint);
        let prefix_width = display_width(dir_prefix);
        let model_available = inner_width
            .saturating_sub(prefix_width)
            .saturating_sub(hint_width);
        let model_value = truncate_for_startup_card(&self.model_label, model_available);
        let gap_width = inner_width
            .saturating_sub(prefix_width)
            .saturating_sub(display_width(&model_value))
            .saturating_sub(hint_width)
            .max(1);
        let dir_max = inner_width.saturating_sub(prefix_width);
        let dir_truncated = truncate_path_middle(&self.directory, dir_max);

        let line_model = Line::from(vec![
            Span::styled(model_prefix, Style::default().fg(TEXT_ACCENT)),
            Span::styled(
                model_value.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(gap_width)),
            Span::styled(hint, Style::default().fg(TEXT_MUTED)),
        ]);
        let line_dir = Line::from(vec![
            Span::styled(dir_prefix, Style::default().fg(TEXT_ACCENT)),
            Span::styled(
                dir_truncated.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]);

        let total_width = inner_width + 2; // account for left margin
        vec![
            top_rule_line("RARA", total_width),
            Line::from(""),
            line_model,
            line_dir,
        ]
    }
}

/// Build a thin top rule line with a centered label, e.g. `── RARA ──────────`.
fn top_rule_line(label: &str, total_width: usize) -> Line<'static> {
    let label_width = display_width(label);
    let used = 4 + label_width; // "── " + label + " "
    let remaining = total_width.saturating_sub(used);
    let line = format!("── {label} {}", "─".repeat(remaining));
    Line::from(Span::styled(line, Style::default().fg(TEXT_MUTED)))
}
