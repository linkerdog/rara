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
};
use crate::tui::state::{ActivePendingInteractionKind, TuiApp};
use crate::tui::sub_agent_display::SUB_AGENT_QUESTION_COLOR;
use crate::tui::theme::*;

pub(crate) struct PlanSummaryCell {
    steps: Vec<(String, String)>,
    explanation: Option<String>,
}

impl PlanSummaryCell {
    pub(crate) fn new(steps: Vec<(String, String)>, explanation: Option<String>) -> Self {
        Self { steps, explanation }
    }
}

impl HistoryCell for PlanSummaryCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        updated_plan_lines(self.steps.as_slice(), self.explanation.as_deref())
    }
}

pub(crate) struct PlanningSuggestionCell {
    text: String,
}

impl PlanningSuggestionCell {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl HistoryCell for PlanningSuggestionCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(section_label(
            "Planning Suggested",
            PHASE_PLANNING,
        ))];
        lines.extend(
            self.text
                .lines()
                .map(|line| Line::from(format!("  {line}"))),
        );
        lines
    }
}

pub(crate) struct PlanModeCell;

impl HistoryCell for PlanModeCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![Line::from(section_label("Plan Mode", PHASE_PLANNING))]
    }
}

pub(crate) fn planning_suggestion_text(app: &TuiApp) -> String {
    status_planning_suggestion_text(app)
}
