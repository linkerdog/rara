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

struct SummaryCell {
    title: &'static str,
    color: Color,
    summary: String,
}

impl SummaryCell {
    fn new(title: &'static str, color: Color, summary: impl Into<String>) -> Self {
        Self {
            title,
            color,
            summary: summary.into(),
        }
    }
}

impl HistoryCell for SummaryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(section_label(self.title, self.color))];
        let mut summary_lines = self.summary.lines();
        while let Some(line) = summary_lines.next() {
            if line.trim_start() == "diff:" {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "diff:",
                        Style::default()
                            .fg(PHASE_PLANNING)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                let diff = summary_lines
                    .map(|line| line.trim_start())
                    .collect::<Vec<_>>()
                    .join("\n");
                lines.extend(render_patch_preview(diff.as_str(), width));
                break;
            }
            lines.push(Line::from(format!("  {line}")));
        }
        lines
    }
}
macro_rules! summary_cell {
    ($name:ident, $title:expr, $color:expr) => {
        pub(crate) struct $name {
            inner: SummaryCell,
        }
        impl $name {
            pub(crate) fn new(summary: impl Into<String>) -> Self {
                Self {
                    inner: SummaryCell::new($title, $color, summary),
                }
            }
        }
        impl HistoryCell for $name {
            fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
                self.inner.display_lines(width)
            }
        }
    };
    ($name:ident, $active_title:expr, $active_color:expr, $done_title:expr, $done_color:expr) => {
        pub(crate) struct $name {
            inner: SummaryCell,
        }
        impl $name {
            pub(crate) fn new(summary: impl Into<String>, active: bool) -> Self {
                let (title, color) = if active {
                    ($active_title, $active_color)
                } else {
                    ($done_title, $done_color)
                };
                Self {
                    inner: SummaryCell::new(title, color, summary),
                }
            }
        }
        impl HistoryCell for $name {
            fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
                self.inner.display_lines(width)
            }
        }
    };
}

summary_cell!(ExploredCell, "Explored", PHASE_EXPLORED);
summary_cell!(RanCell, "Ran", PHASE_RAN);

summary_cell!(
    PlanningCell,
    "Planning",
    PHASE_PLANNING,
    "Planned",
    PHASE_PLANNING
);
summary_cell!(
    ExploringCell,
    "Exploring",
    PHASE_EXPLORING,
    "Explored",
    PHASE_EXPLORED
);
summary_cell!(RunningCell, "Running", PHASE_RUNNING, "Ran", PHASE_RAN);

#[cfg(test)]
mod tests {
    use super::{HistoryCell, SummaryCell};
    use crate::tui::theme::PHASE_RAN;

    #[test]
    fn summary_cell_renders_indented_diff_block_as_patch_preview() {
        let cell = SummaryCell::new(
            "Ran",
            PHASE_RAN,
            "  replace src/main.rs\n  diff:\n  *** Begin Patch\n  *** Update File: src/main.rs\n  @@\n  -old\n  +new\n  *** End Patch",
        );

        let rendered = cell
            .display_lines(100)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("diff:"));
        assert!(rendered.contains("Edited src/main.rs"));
        assert!(rendered.contains("- old"));
        assert!(rendered.contains("+ new"));
    }
}
