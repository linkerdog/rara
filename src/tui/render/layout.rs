use ratatui::layout::{Constraint, Direction, Layout};

use super::bottom_pane::{desired_bottom_pane_height, render_bottom_pane};
use super::overlay::render_overlay;
use super::{render_startup_header, render_transcript, sidebar};
use crate::tui::custom_terminal::Frame;
use crate::tui::pane_geometry::PaneColumns;
use crate::tui::state::TuiApp;

pub fn render(f: &mut Frame, app: &mut TuiApp) {
    app.terminal_width = f.area().width;
    let columns = PaneColumns {
        terminal_width: app.terminal_width,
        sidebar_visible: app.sidebar_visible,
    };
    let bottom_pane_height = desired_bottom_pane_height(app, columns.main_width(), f.area().height);

    if let Some(sidebar_width) = columns.sidebar_width() {
        render_wide(f, app, bottom_pane_height, sidebar_width);
    } else {
        render_narrow(f, app, bottom_pane_height);
    }
    crate::tui::terminal_presentation::project_frame(f.buffer_mut(), app.terminal_capabilities);
}

fn render_narrow(f: &mut Frame, app: &mut TuiApp, bottom_pane_height: u16) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(bottom_pane_height)])
        .split(f.area());

    let transcript_area = render_startup_header(f, app, layout[0]);
    render_transcript(f, app, transcript_area);
    let mut cursor = render_bottom_pane(f, app, layout[1]);

    if let Some(overlay) = app.overlay {
        cursor = render_overlay(f, app, overlay).or(cursor);
    }

    if let Some((x, y)) = cursor {
        f.set_cursor_position((x, y));
    }
}

fn render_wide(f: &mut Frame, app: &mut TuiApp, bottom_pane_height: u16, sidebar_width: u16) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_width), Constraint::Fill(1)])
        .split(f.area());

    sidebar::render_sidebar(f, app, layout[0]);

    // Main transcript panel — same vertical split as narrow mode.
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(bottom_pane_height)])
        .split(layout[1]);

    let transcript_area = render_startup_header(f, app, main[0]);
    render_transcript(f, app, transcript_area);
    let mut cursor = render_bottom_pane(f, app, main[1]);

    if let Some(overlay) = app.overlay {
        cursor = render_overlay(f, app, overlay).or(cursor);
    }

    if let Some((x, y)) = cursor {
        f.set_cursor_position((x, y));
    }
}
