// Activity bar — renders the top line of the bottom pane from an ActivityView.
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::super::custom_terminal::Frame;
use super::super::helpers::badge;
use crate::tui::theme::{STATUS_INFO, TEXT_ACCENT, TEXT_MUTED};

pub(super) fn render_activity_bar(f: &mut Frame, view: &super::view::ActivityView, area: Rect) {
    let mut spans: Vec<Span<'_>> = Vec::new();
    let bold = Style::default().add_modifier(Modifier::BOLD);
    spans.push(Span::styled(view.label.as_str(), bold.fg(view.label_color)));

    if view.plan_badge {
        spans.push(Span::raw(" "));
        spans.push(badge("mode", "plan", TEXT_ACCENT));
    }
    if view.perm_badge {
        spans.push(Span::raw(" "));
        spans.push(badge("perm", view.perm_label, STATUS_INFO));
    }

    if let Some((goal_label, goal_color)) = view.goal_label {
        spans.push(Span::raw("  "));
        spans.push(badge("goal", goal_label, goal_color));
        if let Some(ref detail) = view.goal_detail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                detail.as_str(),
                Style::default().fg(TEXT_MUTED),
            ));
        }
    }

    if !view.detail.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            view.detail.as_str(),
            Style::default().fg(TEXT_ACCENT),
        ));
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line);
    f.render_widget(para, area);
}
