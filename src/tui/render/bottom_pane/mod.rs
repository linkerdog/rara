// Bottom pane orchestrator — layout split and public API.
pub(super) mod composer;
pub(super) mod status;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::custom_terminal::Frame;
use super::super::interaction_text::pending_interaction_hint_text;
use super::super::queued_input::{pending_follow_up_hint, queued_follow_up_hint};
use super::super::state::char_offset_to_byte_index;
use super::super::state::{ActivePendingInteractionKind, GoalStatus, TaskKind, TuiApp};
use super::badge;
use crate::tui::format::cache_hit_rate_label;
use crate::tui::theme::*;

const COMPOSER_TAB_WIDTH: usize = 4;
const BOTTOM_PANE_BG: Color = SURFACE_BOTTOM_PANE_BG;

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
    render_bottom_pane_inner(f, app, area)
}

fn render_bottom_pane_inner(f: &mut Frame, app: &mut TuiApp, area: Rect) -> Option<(u16, u16)> {
    let composer_height = area.height.saturating_sub(2).max(3);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);
    status::render_activity_bar(f, app, chunks[0]);
    let cursor = composer::render_composer(f, app, chunks[1]);
    status::render_footer(f, app, chunks[2]);
    cursor
}


pub(super) fn bottom_pane_style() -> Style {
    Style::default().bg(BOTTOM_PANE_BG)
}


