// Bottom pane orchestrator — layout split and public API.
mod activity;
pub(super) mod composer;
mod footer;
mod view;
mod view_builder;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use super::super::custom_terminal::Frame;
use super::super::state::TuiApp;
use crate::tui::theme::{SURFACE_BOTTOM_PANE_BG, TEXT_PRIMARY, TEXT_SECONDARY};

const BOTTOM_PANE_BG: Color = SURFACE_BOTTOM_PANE_BG;

pub(crate) fn desired_viewport_height(app: &TuiApp, width: u16, rows: u16) -> u16 {
    if app.overlay.is_some() {
        return rows.max(1);
    }
    let bottom_pane_height = desired_bottom_pane_height(app, width, rows);
    let has_active_content =
        !app.active_turn.entries.is_empty() || app.bottom_pane.has_pending_planning_suggestion();
    if !app.has_any_transcript() && !has_active_content {
        return rows.max(1);
    }
    rows.saturating_sub(bottom_pane_height).max(1)
}

pub(crate) fn desired_bottom_pane_height(app: &TuiApp, width: u16, rows: u16) -> u16 {
    let composer_rows = composer::desired_composer_height(app, width, rows);
    let panel_rows = if app
        .active_pending_interaction()
        .is_some_and(|p| p.kind != super::super::state::ActivePendingInteractionKind::RequestInput)
    {
        5
    } else {
        0
    };
    let total = composer_rows.saturating_add(2).saturating_add(panel_rows);
    let max = rows.max(1);
    let min = 5.min(max);
    total.clamp(min, max)
}

pub(super) fn bottom_pane_style() -> Style {
    Style::default().bg(BOTTOM_PANE_BG)
}

pub(super) fn render_bottom_pane(
    f: &mut Frame,
    app: &mut TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let view = view_builder::build_bottom_pane_view(app, area.width, area.height);

    f.render_widget(Block::default().style(bottom_pane_style()), area);

    let has_panel = view.interaction_panel.is_some();
    let composer_height = area
        .height
        .saturating_sub(2 + if has_panel { 5 } else { 0 })
        .max(3);
    let constraints = if has_panel {
        vec![
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    activity::render_activity_bar(f, &view.activity, chunks[0]);
    let composer_idx = if has_panel { 2 } else { 1 };
    let footer_idx = composer_idx + 1;
    if let Some(panel) = &view.interaction_panel {
        render_interaction_panel(f, panel, chunks[1]);
    }
    let cursor = composer::render_composer(f, app, chunks[composer_idx]);
    footer::render_footer(f, &view.footer, chunks[footer_idx]);
    cursor
}

fn render_interaction_panel(f: &mut Frame, panel: &view::InteractionPanelView, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("# {}", panel.title),
            Style::default().fg(panel.color),
        ),
    ]));
    lines.push(Line::from(""));
    if panel.detail.is_empty() {
        for (i, action) in panel.actions.iter().enumerate() {
            lines.push(interaction_action_row(panel, i, action));
        }
    } else {
        let detail_rows = usize::from(area.height).saturating_sub(3);
        for line in panel.detail.lines().take(detail_rows) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line.to_string(), Style::default().fg(TEXT_SECONDARY)),
            ]));
        }
        lines.push(interaction_action_line(panel));
    }

    let para = Paragraph::new(lines)
        .style(bottom_pane_style())
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
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
