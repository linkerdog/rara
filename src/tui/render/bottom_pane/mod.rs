// Bottom pane orchestrator — layout split and public API.
mod activity;
pub(super) mod composer;
mod footer;
mod view;
mod view_builder;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::Block,
};

use super::super::custom_terminal::Frame;
use super::super::state::{ActivePendingInteractionKind, TuiApp};
use super::badge;
use crate::tui::theme::*;

pub(crate) fn desired_viewport_height(app: &TuiApp, width: u16, rows: u16) -> u16 {
    if app.overlay.is_some() {
        return rows.max(1);
    }

    if app.transcript_scroll > 0 {
        return rows.max(1);
    }

    let bottom_pane_height = desired_bottom_pane_height(app, width, rows);
    let has_active_content =
        !app.active_turn.entries.is_empty() || app.has_pending_planning_suggestion();
    if !app.has_any_transcript() && !has_active_content {
        return rows.max(1);
    }

    rows.saturating_sub(bottom_pane_height).max(1)
}

pub(crate) fn desired_bottom_pane_height(app: &TuiApp, width: u16, rows: u16) -> u16 {
    let composer_rows = composer::desired_composer_height(app, width, rows);
    let total = composer_rows.saturating_add(2);
    let max = rows.max(1);
    let min = 5.min(max);
    total.clamp(min, max)
}

pub(super) fn render_bottom_pane(
    f: &mut Frame,
    app: &mut TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let view = view_builder::build_bottom_pane_view(app, area.width, area.height);
    // Highlight the bottom pane background when a pending interaction needs attention.
    let style = if let Some(pending) = app.active_pending_interaction() {
        let color = match pending.kind {
            ActivePendingInteractionKind::ShellApproval => STATUS_WARNING,
            ActivePendingInteractionKind::PlanApproval => TEXT_ACCENT,
            _ => STATUS_SUCCESS,
        };
        Style::default().bg(color).fg(Color::Black)
    } else {
        bottom_pane_style()
    };
    f.render_widget(Block::default().style(style), area);
    render_bottom_pane_inner(f, app, area, &view)
}

fn render_bottom_pane_inner(
    f: &mut Frame,
    app: &mut TuiApp,
    area: Rect,
    view: &view::BottomPaneView,
) -> Option<(u16, u16)> {
    let composer_height = area.height.saturating_sub(2).max(3);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);
    activity::render_activity_bar(f, &view.activity, chunks[0]);
    let cursor = composer::render_composer(f, app, chunks[1]);
    footer::render_footer(f, &view.footer, chunks[2]);
    cursor
}

pub(super) fn bottom_pane_style() -> Style {
    Style::default().bg(SURFACE_BOTTOM_PANE_BG)
}
