// Auto-split from cells_components.rs
use std::path::Path;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::tool_progress::tool_progress_lines;
use super::{HistoryCell, InteractionCompletionKind};
use crate::tui::interaction_text::{
    pending_interaction_card_title, status_planning_suggestion_text,
};
use crate::tui::markdown_render::render_markdown_text_with_width;
use crate::tui::plan_display::updated_plan_lines;
use crate::tui::queued_input::{
    QueuedFollowUpSection, pending_follow_up_heading, queued_follow_up_heading,
};
use crate::tui::render::diff::render_patch_preview;
use crate::tui::render::{
    display_width, formatted_message_lines, prefixed_message_lines, rendered_markdown_lines,
    section_label, startup_card_inner_width, truncate_for_startup_card, truncate_path_middle,
    with_border,
};
use crate::tui::state::{ActivePendingInteractionKind, TuiApp};
use crate::tui::sub_agent_display::SUB_AGENT_QUESTION_COLOR;
use crate::tui::theme::*;

pub(crate) struct MessageCell<'a> {
    role: &'a str,
    message: &'a str,
    max_lines: usize,
    cwd: Option<&'a Path>,
}

impl<'a> MessageCell<'a> {
    pub(crate) fn new(
        role: &'a str,
        message: &'a str,
        max_lines: usize,
        cwd: Option<&'a Path>,
    ) -> Self {
        Self {
            role,
            message,
            max_lines,
            cwd,
        }
    }
}

impl HistoryCell for MessageCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.message.contains("*** Begin Patch") {
            let mut lines = Vec::new();
            if !self.role.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    self.role.to_string(),
                    Style::default()
                        .fg(TEXT_SECONDARY)
                        .add_modifier(Modifier::ITALIC),
                )]));
            }
            lines.extend(render_patch_preview(self.message, width));
            return lines;
        }
        formatted_message_lines(self.role, self.message, self.max_lines, self.cwd)
    }
}

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

        let model_label = "model:";
        let directory_label = "directory:";
        let label_width = directory_label.len();
        let model_prefix = format!("{model_label:<label_width$} ");
        let hint = "/model to change";
        let hint_width = display_width(hint);
        let model_prefix_width = display_width(&model_prefix);
        let model_available_width = inner_width
            .saturating_sub(model_prefix_width)
            .saturating_sub(1)
            .saturating_sub(hint_width);
        let model_value = truncate_for_startup_card(&self.model_label, model_available_width);
        let model_value_width = display_width(&model_value);
        let gap_width = inner_width
            .saturating_sub(model_prefix_width)
            .saturating_sub(model_value_width)
            .saturating_sub(hint_width)
            .max(1);
        let directory_prefix = format!("{directory_label:<label_width$} ");
        let directory_max_width = inner_width.saturating_sub(display_width(&directory_prefix));

        let lines = vec![
            Line::from(vec![Span::from(">_ "), Span::from("RARA")]),
            Line::from(""),
            Line::from(vec![
                Span::from(model_prefix),
                Span::from(model_value),
                Span::from(" ".repeat(gap_width)),
                Span::from(hint),
            ]),
            Line::from(vec![
                Span::from(directory_prefix),
                Span::from(truncate_path_middle(&self.directory, directory_max_width)),
            ]),
        ];

        with_border(lines, inner_width)
    }
}
