use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::Block,
};

use super::{activity, composer, footer, interaction, view_builder};
use crate::tui::custom_terminal::Frame;
use crate::tui::state::TuiApp;
use crate::tui::theme::SURFACE_BOTTOM_PANE_BG;

pub(crate) fn desired_bottom_pane_height(app: &TuiApp, width: u16, rows: u16) -> u16 {
    let panel = view_builder::build_interaction_panel(app);
    let composer_rows = if panel.is_some() {
        3
    } else {
        composer::desired_composer_height(app, width, rows)
    };
    let panel_rows = panel.map_or(0, |panel| {
        interaction::desired_height(&panel, width).min(panel_height_budget(rows))
    });
    let total = composer_rows.saturating_add(2).saturating_add(panel_rows);
    let max = rows.max(1);
    let min = 5.min(max);
    total.clamp(min, max)
}

fn panel_height_budget(rows: u16) -> u16 {
    // Preserve composer space on normal screens, but give short screens enough
    // room for stacked actions and at least one scrollable command row.
    rows.saturating_sub(5).max(rows.saturating_sub(2).min(8))
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
    if view
        .interaction_panel
        .as_ref()
        .is_none_or(|panel| panel.shell_approval.is_none())
    {
        app.bottom_pane.approval_details = Default::default();
    }
    f.render_widget(Block::default().style(bottom_pane_style()), area);

    let panel_height = view.interaction_panel.as_ref().map_or(0, |panel| {
        interaction::desired_height(panel, area.width).min(panel_height_budget(area.height))
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
        interaction::render(f, panel, panel_area, &mut app.bottom_pane.approval_details);
    }
    let cursor = if composer_area.height > 0 {
        composer::render_composer(f, app, composer_area)
    } else {
        None
    };
    footer::render_footer(f, &view.footer, footer_area);
    cursor
}
