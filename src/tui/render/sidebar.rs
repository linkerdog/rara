// Wide-screen sidebar (≥120 cols) rendered alongside the main transcript pane.
// Layout draws a 38-column panel on the left split by a vertical border,

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::tui::custom_terminal::Frame;
use crate::tui::state::TuiApp;
use crate::tui::status_display::context_sidebar_summary;
use crate::tui::theme::*;

/// Width allocated to the sidebar when the terminal is wide enough.
pub(crate) const SIDEBAR_WIDTH: u16 = 38;

/// Render the wide-screen sidebar into `area`.
pub(crate) fn render_sidebar(f: &mut Frame, app: &TuiApp, area: Rect) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Reserve bottom row for version / status line.
    let sub = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(1)])
        .split(inner);

    let body_area = sub[0];
    let footer_area = sub[1];

    let mut lines: Vec<Line<'static>> = Vec::new();

    push_session_info(&mut lines, app);
    push_model_badge(&mut lines, app);
    lines.push(Line::from(""));
    push_context_summary(&mut lines, app);
    lines.push(Line::from(""));
    if push_todo_section(&mut lines, app) {
        lines.push(Line::from(""));
    }
    push_child_sessions(&mut lines, app);
    lines.push(Line::from(""));

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body_area);

    let version_line = version_footer_line();
    f.render_widget(
        Paragraph::new(Line::from(version_line)).style(Style::default().fg(TEXT_MUTED)),
        footer_area,
    );
}

fn push_session_info(lines: &mut Vec<Line<'static>>, app: &TuiApp) {
    let title = if app.snapshot.session_id.is_empty() {
        "RARA".to_string()
    } else {
        // Shorten session id for display.
        let session_id = &app.snapshot.session_id;
        if session_id.len() > 14 {
            format!(
                "{}…{}",
                &session_id[..8],
                &session_id[session_id.len().saturating_sub(4)..]
            )
        } else {
            session_id.clone()
        }
    };

    lines.push(Line::from(Span::styled(
        title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));

    // Compact cwd (+ branch if available) on a single line.
    let cwd = super::display_directory_for_startup(app);
    let location = if app.snapshot.branch.is_empty() {
        cwd
    } else {
        format!("{} :: {}", cwd, app.snapshot.branch)
    };
    lines.push(Line::from(Span::styled(
        location,
        Style::default().fg(TEXT_MUTED),
    )));
}
fn push_model_badge(lines: &mut Vec<Line<'static>>, app: &TuiApp) {
    let provider = &app.config.provider;
    let provider = provider.as_str();

    let model = app
        .config
        .model
        .as_deref()
        .filter(|m| !m.is_empty())
        .unwrap_or("default");

    let badge = format!(" {provider} / {model} ");

    lines.push(Line::from(Span::styled(
        badge,
        Style::default()
            .fg(BADGE_FG_DARK)
            .add_modifier(Modifier::BOLD),
    )));

    // Reasoning effort if applicable
    if let Some(ref effort) = app.config.reasoning_effort {
        let effort = effort.trim();
        if !effort.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  reasoning  {effort}"),
                Style::default().fg(TEXT_MUTED),
            )));
        }
    }
}

fn push_context_summary(lines: &mut Vec<Line<'static>>, app: &TuiApp) {
    lines.push(Line::from(super::section_label("Context", TEXT_SECONDARY)));

    let snap = &app.snapshot;
    lines.push(Line::from(Span::styled(
        context_sidebar_summary(snap),
        Style::default().fg(TEXT_MUTED),
    )));
}

fn push_todo_section(lines: &mut Vec<Line<'static>>, app: &TuiApp) -> bool {
    let todo = &app.snapshot.todo;
    if todo.summary.total == 0 {
        return false;
    }

    lines.push(Line::from(super::section_label("Todo", TEXT_SECONDARY)));
    let open = todo.summary.pending + todo.summary.in_progress;
    lines.push(Line::from(Span::styled(
        format!(
            "{}/{} done · {} open",
            todo.summary.completed, todo.summary.total, open
        ),
        Style::default().fg(TEXT_MUTED),
    )));

    if let Some(active) = todo.summary.active_item.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("Active: {active}"),
            Style::default().fg(INTERACTION_SUB_AGENT),
        )));
    }

    for (_, status, content) in todo.items.iter().take(4) {
        lines.push(Line::from(Span::styled(
            format!("{} {}", todo_status_marker(status), content.trim()),
            todo_status_style(status),
        )));
    }

    if todo.items.len() > 4 {
        lines.push(Line::from(Span::styled(
            format!("... and {} more", todo.items.len() - 4),
            Style::default().fg(TEXT_MUTED),
        )));
    }

    true
}

fn todo_status_marker(status: &str) -> &'static str {
    match status {
        "in_progress" => "[>]",
        "completed" => "[x]",
        "cancelled" => "[-]",
        _ => "[ ]",
    }
}

fn todo_status_style(status: &str) -> Style {
    match status {
        "in_progress" => Style::default().fg(INTERACTION_SUB_AGENT),
        "completed" => Style::default().fg(STATUS_SUCCESS),
        "cancelled" => Style::default().fg(TEXT_MUTED),
        _ => Style::default().fg(TEXT_SECONDARY),
    }
}

fn push_child_sessions(lines: &mut Vec<Line<'static>>, app: &TuiApp) {
    let child_count = app.snapshot.pending_interactions.len();
    if child_count == 0 {
        return;
    }

    lines.push(Line::from(super::section_label(
        "Sub-agents",
        TEXT_SECONDARY,
    )));

    // Show running sub-agents up to 5.
    for pi in app.snapshot.pending_interactions.iter().take(5) {
        let label = format!("  {} (running)", pi.title);
        lines.push(Line::from(Span::styled(
            label,
            Style::default().fg(INTERACTION_SUB_AGENT),
        )));
    }

    if child_count > 5 {
        lines.push(Line::from(Span::styled(
            format!("  ... and {} more", child_count - 5),
            Style::default().fg(TEXT_MUTED),
        )));
    }
}

fn version_footer_line() -> Span<'static> {
    let version = option_env!("CARGO_PKG_VERSION").unwrap_or("dev");
    Span::styled(
        format!("• rara v{version}"),
        Style::default().fg(TEXT_MUTED),
    )
}

#[cfg(test)]
#[path = "sidebar_tests.rs"]
mod tests;
