// Activity bar — renders the top line of the bottom pane from an ActivityView.
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::super::custom_terminal::Frame;
use super::badge;
use super::bottom_pane_style;
use super::view::ActivityView;

pub(super) fn render_activity_bar(f: &mut Frame, view: &ActivityView, area: Rect) {
    let mut spans = vec![Span::styled(
        view.label.as_str(),
        Style::default()
            .fg(view.label_color)
            .add_modifier(Modifier::BOLD),
    )];

    if view.plan_badge {
        spans.push(Span::raw("  "));
        spans.push(badge("mode", "plan", crate::tui::theme::TEXT_ACCENT));
    }
    if view.perm_badge {
        spans.push(Span::raw("  "));
        spans.push(badge(
            "perm",
            view.perm_label,
            crate::tui::theme::STATUS_INFO,
        ));
    }
    if let Some(goal) = &view.goal {
        spans.push(Span::raw("  "));
        let (goal_label, goal_color) = match goal.status {
            super::super::super::state::GoalStatus::Pursuing => {
                ("pursuing", crate::tui::theme::STATUS_INFO)
            }
            super::super::super::state::GoalStatus::Paused => {
                ("paused", crate::tui::theme::STATUS_WARNING)
            }
            super::super::super::state::GoalStatus::Complete => {
                ("done", crate::tui::theme::STATUS_SUCCESS)
            }
            super::super::super::state::GoalStatus::BudgetLimited => {
                ("budget", crate::tui::theme::STATUS_WARNING)
            }
        };
        spans.push(badge("goal", goal_label, goal_color));
        let goal_detail = if let Some(budget) = goal.token_budget {
            format!(
                "t{} · {}/{} tokens · {} left",
                goal.turns_completed,
                goal.tokens_used,
                budget,
                goal.remaining_tokens().unwrap_or(0)
            )
        } else {
            format!("t{} · {} tokens", goal.turns_completed, goal.tokens_used)
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            goal_detail,
            Style::default().fg(crate::tui::theme::TEXT_MUTED),
        ));
    }
    if !view.detail.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            view.detail.as_str(),
            Style::default().fg(crate::tui::theme::TEXT_SECONDARY),
        ));
    }
    let status = Paragraph::new(Line::from(spans)).style(bottom_pane_style());
    f.render_widget(status, area);
}
