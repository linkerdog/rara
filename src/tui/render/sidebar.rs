// Wide-screen sidebar (≥120 cols) rendered alongside the main transcript pane.
// Layout draws a 38-column panel on the left split by a vertical border,

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::local_model_server::LocalModelServerState;
use crate::tui::custom_terminal::Frame;
use crate::tui::state::{GoalStatus, TuiApp};
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
    push_lsp_status(&mut lines, app);
    lines.push(Line::from(""));
    push_local_model_section(&mut lines, app);
    lines.push(Line::from(""));
    if push_plan_section(&mut lines, app) {
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

fn push_lsp_status(lines: &mut Vec<Line<'static>>, app: &TuiApp) {
    lines.push(Line::from(super::section_label("LSP", TEXT_SECONDARY)));
    let Some(manager) = app.lsp_manager.as_ref() else {
        lines.push(Line::from(Span::styled(
            "not initialized",
            Style::default().fg(TEXT_MUTED),
        )));
        return;
    };

    let snapshot = manager.status_snapshot();
    if !snapshot.enabled {
        lines.push(Line::from(Span::styled(
            "disabled by RARA_LSP",
            Style::default().fg(TEXT_MUTED),
        )));
        return;
    }

    let detected = snapshot
        .servers
        .iter()
        .filter(|server| server.detected)
        .collect::<Vec<_>>();
    if detected.is_empty() {
        lines.push(Line::from(Span::styled(
            "no project server detected",
            Style::default().fg(TEXT_MUTED),
        )));
        return;
    }

    let running = detected.iter().filter(|server| server.running).count();
    let available = detected.iter().filter(|server| server.available).count();
    let status = if running > 0 {
        format!(
            "{} running · {} diagnostics",
            running, snapshot.diagnostic_count
        )
    } else if available > 0 {
        format!("idle · {available} available")
    } else if detected.iter().any(|server| !server.checked) {
        "detected · not started".to_string()
    } else {
        "server missing".to_string()
    };
    lines.push(Line::from(Span::styled(
        status,
        Style::default().fg(TEXT_MUTED),
    )));

    for server in detected.iter().take(3) {
        let marker = if server.running {
            "[>]"
        } else if server.available {
            "[ ]"
        } else if !server.checked {
            "[?]"
        } else {
            "[!]"
        };
        let style = if server.running {
            Style::default().fg(INTERACTION_SUB_AGENT)
        } else if server.available {
            Style::default().fg(TEXT_SECONDARY)
        } else if !server.checked {
            Style::default().fg(TEXT_MUTED)
        } else {
            Style::default().fg(STATUS_WARNING)
        };
        lines.push(Line::from(Span::styled(
            format!("{marker} {}", server.name),
            style,
        )));
        // Add a hint below each server showing its current phase.
        if !server.running {
            lines.push(Line::from(Span::styled(
                "    ready",
                Style::default().fg(TEXT_MUTED),
            )));
        }
    }

    if let Some(error) = snapshot.last_error {
        lines.push(Line::from(Span::styled(
            format!("last error: {error}"),
            Style::default().fg(STATUS_WARNING),
        )));
    }
}

fn push_local_model_section(lines: &mut Vec<Line<'static>>, app: &TuiApp) {
    let status = &app.local_model_server;
    lines.push(Line::from(super::section_label(
        "Local Model",
        TEXT_SECONDARY,
    )));
    let (marker, style) = match status.state {
        LocalModelServerState::Ready => ("● ready", Style::default().fg(STATUS_SUCCESS)),
        LocalModelServerState::Starting
        | LocalModelServerState::WaitingForServer
        | LocalModelServerState::CreatingVenv
        | LocalModelServerState::InstallingDependencies
        | LocalModelServerState::PreparingModel => {
            ("○ preparing …", Style::default().fg(STATUS_WARNING))
        }
        LocalModelServerState::PreparedButStopped => {
            ("○ stopped", Style::default().fg(TEXT_SECONDARY))
        }
        LocalModelServerState::SetupRequired => {
            ("○ setup required", Style::default().fg(TEXT_MUTED))
        }
        LocalModelServerState::Error => ("✗ error", Style::default().fg(STATUS_ERROR)),
    };
    lines.push(Line::from(Span::styled(marker, style)));
    if !status.model.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", status.model),
            Style::default().fg(TEXT_MUTED),
        )));
    }
}

fn push_plan_section(lines: &mut Vec<Line<'static>>, app: &TuiApp) -> bool {
    let plan_steps = &app.snapshot.plan_steps;
    let goal = &app.goal;
    if plan_steps.is_empty() && goal.is_none() {
        return false;
    }
    lines.push(Line::from(super::section_label("Plan", TEXT_SECONDARY)));
    if let Some(goal) = goal.as_ref() {
        let mark = match goal.status {
            GoalStatus::Pursuing => "\u{1f3af}",
            GoalStatus::Complete => "\u{2705}",
            GoalStatus::Paused => "\u{23f8}",
            GoalStatus::BudgetLimited => "\u{23f1}",
        };
        let style = match goal.status {
            GoalStatus::Pursuing => STATUS_WARNING,
            GoalStatus::Complete => STATUS_SUCCESS,
            _ => TEXT_MUTED,
        };
        lines.push(Line::from(Span::styled(
            format!("{} {}", mark, goal.objective),
            Style::default().fg(style),
        )));
        if goal.tokens_used > 0 {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} tokens, {} turns",
                    goal.tokens_used, goal.turns_completed
                ),
                Style::default().fg(TEXT_MUTED),
            )));
        }
    }
    if !plan_steps.is_empty() {
        let total = plan_steps.len();
        let done = plan_steps
            .iter()
            .filter(|(status, _)| status == "done")
            .count();
        lines.push(Line::from(Span::styled(
            format!("{}/{} done", done, total),
            Style::default().fg(TEXT_MUTED),
        )));
        for (st, d) in plan_steps.iter().take(8) {
            let (m, s) = match st.as_str() {
                "done" => ("[x]", STATUS_SUCCESS),
                "in_progress" => ("[>]", STATUS_WARNING),
                "cancelled" => ("[-]", TEXT_MUTED),
                _ => ("[ ]", TEXT_SECONDARY),
            };
            lines.push(Line::from(Span::styled(
                format!("{} {}", m, d.trim()),
                Style::default().fg(s),
            )));
        }
        if plan_steps.len() > 8 {
            lines.push(Line::from(Span::styled(
                format!("... {} more", plan_steps.len() - 8),
                Style::default().fg(TEXT_MUTED),
            )));
        }
    }
    true
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
