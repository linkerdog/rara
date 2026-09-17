use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::Block,
};

use super::{activity, composer, footer, interaction, view_builder};
use crate::tui::custom_terminal::Frame;
use crate::tui::state::TuiApp;
use crate::tui::theme::SURFACE_BOTTOM_PANE_BG;

pub(crate) fn desired_viewport_height(app: &TuiApp, width: u16, rows: u16) -> u16 {
    if app.overlay.is_some() || app.active_pending_interaction().is_some() {
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
    let panel = view_builder::build_interaction_panel(app);
    let composer_rows = if panel.is_some() {
        3
    } else {
        composer::desired_composer_height(app, width, rows)
    };
    let panel_rows = panel.map_or(0, |panel| interaction::desired_height(&panel, width));
    let total = composer_rows.saturating_add(2).saturating_add(panel_rows);
    let max = rows.max(1);
    let min = 5.min(max);
    total.clamp(min, max)
}

pub(in crate::tui::render) fn bottom_pane_style() -> Style {
    Style::default().bg(SURFACE_BOTTOM_PANE_BG)
}

pub(in crate::tui::render) fn render_bottom_pane(
    f: &mut Frame,
    app: &mut TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let view = view_builder::build_bottom_pane_view(app, area.width, area.height);
    f.render_widget(Block::default().style(bottom_pane_style()), area);

    let panel_height = view.interaction_panel.as_ref().map_or(0, |panel| {
        interaction::desired_height(panel, area.width).min(area.height.saturating_sub(2))
    });
    let composer_height = area.height.saturating_sub(2 + panel_height);
    let [activity_area, panel_area, composer_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(panel_height),
        Constraint::Length(composer_height),
        Constraint::Length(1),
    ])
    .areas(area);
    activity::render_activity_bar(f, &view.activity, activity_area);
    if let Some(panel) = &view.interaction_panel {
        interaction::render(f, panel, panel_area);
    }
    let cursor = if composer_area.height > 0 {
        composer::render_composer(f, app, composer_area)
    } else {
        None
    };
    footer::render_footer(f, &view.footer, footer_area);
    cursor
}
