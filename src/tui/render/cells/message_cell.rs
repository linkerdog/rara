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
use crate::tui::render::diff::render_message_diff_preview;
use crate::tui::render::{
    formatted_message_lines, prefixed_message_lines, prefixed_tail_message_lines,
    rendered_markdown_lines, section_label,
};
use crate::tui::state::{ActivePendingInteractionKind, TuiApp};
use crate::tui::sub_agent_display::SUB_AGENT_QUESTION_COLOR;
use crate::tui::theme::*;

pub(crate) struct MessageCell<'a> {
    role: &'a str,
    message: &'a str,
    max_lines: usize,
    cwd: Option<&'a Path>,
    window: MessageWindow,
}

#[derive(Clone, Copy)]
enum MessageWindow {
    HeadTail,
    Tail,
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
            window: MessageWindow::HeadTail,
        }
    }

    pub(crate) fn new_tail(
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
            window: MessageWindow::Tail,
        }
    }
}

impl HistoryCell for MessageCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if let Some(lines) = render_message_diff_preview(Some(self.role), self.message, width) {
            return lines;
        }
        if matches!(self.window, MessageWindow::Tail) {
            return prefixed_tail_message_lines(self.role, self.message, self.max_lines);
        }
        formatted_message_lines(self.role, self.message, self.max_lines, self.cwd)
    }
}
