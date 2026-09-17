use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{bottom_pane_style, composer, view};
use crate::tui::custom_terminal::Frame;
use crate::tui::display_sanitize::sanitize_display_text;
use crate::tui::theme::{TEXT_PRIMARY, TEXT_SECONDARY};

pub(super) fn desired_height(panel: &view::InteractionPanelView, width: u16) -> u16 {
    let preview_rows = detail_lines(panel, width).len().min(2);
    (2 + preview_rows + action_lines(panel, width).len()) as u16
}

pub(super) fn render(f: &mut Frame, panel: &view::InteractionPanelView, area: Rect) {
    let actions = action_lines(panel, area.width);
    let action_height = (actions.len() as u16).min(area.height);
    let [preview_area, action_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(action_height)]).areas(area);
    let mut preview = vec![
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("# {}", panel.title),
                Style::default().fg(panel.color),
            ),
        ]),
        Line::from(""),
    ];
    let details = detail_lines(panel, area.width);
    let preview_budget = usize::from(preview_area.height.saturating_sub(2));
    let clipped = details.len() > preview_budget;
    preview.extend(details.into_iter().take(preview_budget));
    if clipped && preview_budget > 0 {
        *preview.last_mut().expect("preview row") = Line::styled(
            "  … (details truncated)",
            Style::default().fg(TEXT_SECONDARY),
        );
    }
    f.render_widget(
        Paragraph::new(preview).style(bottom_pane_style()),
        preview_area,
    );
    // On extremely short terminals, keep the selected vertical action in view.
    let scroll = if actions.len() > usize::from(action_height) {
        panel
            .selected
            .saturating_sub(usize::from(action_height.saturating_sub(1))) as u16
    } else {
        0
    };
    f.render_widget(
        Paragraph::new(actions)
            .style(bottom_pane_style())
            .scroll((scroll, 0)),
        action_area,
    );
}

fn detail_lines(panel: &view::InteractionPanelView, width: u16) -> Vec<Line<'static>> {
    if panel.detail.is_empty() {
        return Vec::new();
    }
    composer::wrapped_text_rows(
        &sanitize_display_text(&panel.detail),
        width,
        Some("  "),
        Some("  "),
    )
    .into_iter()
    .map(|row| Line::styled(row, Style::default().fg(TEXT_SECONDARY)))
    .collect()
}

fn action_lines(panel: &view::InteractionPanelView, width: u16) -> Vec<Line<'static>> {
    let inline = interaction_action_line(panel);
    if !panel.detail.is_empty() && inline.width() <= usize::from(width) {
        vec![inline]
    } else {
        panel
            .actions
            .iter()
            .enumerate()
            .map(|(index, action)| interaction_action_row(panel, index, action))
            .collect()
    }
}

fn interaction_action_row(
    panel: &view::InteractionPanelView,
    index: usize,
    action: &view::InteractionAction,
) -> Line<'static> {
    let selected = index == panel.selected;
    let marker = if selected { "▸ " } else { "  " };
    let label_style = if selected {
        Style::default()
            .fg(TEXT_PRIMARY)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_SECONDARY)
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            marker,
            Style::default()
                .fg(panel.color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{}]", action.key),
            Style::default()
                .fg(panel.color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(action.label, label_style),
    ])
}

fn interaction_action_line(panel: &view::InteractionPanelView) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    for (index, action) in panel.actions.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("    "));
        }
        let selected = index == panel.selected;
        let marker = if selected { "▸" } else { " " };
        let label_style = if selected {
            Style::default()
                .fg(TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT_SECONDARY)
        };
        spans.push(Span::styled(
            marker,
            Style::default()
                .fg(panel.color)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("[{}]", action.key),
            Style::default()
                .fg(panel.color)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(action.label, label_style));
    }
    Line::from(spans)
}
