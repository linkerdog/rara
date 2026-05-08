// Wide-screen sidebar (≥120 cols) rendered alongside the main transcript pane.
// Layout draws a 38-column panel on the left split by a vertical border,
// showing session identity, model badge, context budget bar, and status.
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::tui::custom_terminal::Frame;
use crate::tui::state::TuiApp;
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
    lines.push(Line::from(""));
    push_model_badge(&mut lines, app);
    lines.push(Line::from(""));
    push_budget_bar(&mut lines, app, body_area.width);
    lines.push(Line::from(""));
    push_child_sessions(&mut lines, app);
    lines.push(Line::from(""));
    push_context_summary(&mut lines, app);

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
        // Shorten session id for display: first 12 chars.
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

    lines.push(Line::from(Span::styled(
        format!("cwd  {}", super::display_directory_for_startup(app)),
        Style::default().fg(TEXT_SECONDARY),
    )));

    if !app.snapshot.branch.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("branch  {}", app.snapshot.branch),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
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
                format!("  reasoning  {}", effort),
                Style::default().fg(TEXT_MUTED),
            )));
        }
    }
}

fn push_budget_bar(lines: &mut Vec<Line<'static>>, app: &TuiApp, width: u16) {
    let snap = &app.snapshot;
    let Some(context_window) = snap.context_window_tokens else {
        return;
    };

    let bar_width = width.saturating_sub(2).max(10) as usize;

    // Collect budget segments: (label, tokens, color)
    struct Segment {
        label: &'static str,
        tokens: usize,
        color: Color,
    }

    let segments = [
        Segment {
            label: "sys",
            tokens: snap.stable_instructions_budget,
            color: BUDGET_SYSTEM,
        },
        Segment {
            label: "ws",
            tokens: snap.workspace_prompt_budget,
            color: BUDGET_WORKSPACE,
        },
        Segment {
            label: "act",
            tokens: snap.active_turn_budget,
            color: BUDGET_ACTIVE,
        },
        Segment {
            label: "hist",
            tokens: snap.compacted_history_budget,
            color: BUDGET_HISTORY,
        },
        Segment {
            label: "mem",
            tokens: snap.retrieved_memory_budget,
            color: BUDGET_MEMORY,
        },
    ];

    let used: usize = segments.iter().map(|s| s.tokens).sum();
    let free = context_window.saturating_sub(used);

    let all_widths: Vec<(usize, Color)> = segments
        .iter()
        .map(|s| (s.tokens, s.color))
        .chain(std::iter::once((free, BUDGET_FREE)))
        .collect();

    // Build a single-line bar from colored spans.
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Scale each segment by bar_width relative to context_window.
    let mut drawn = 0usize;
    for (idx, (tokens, color)) in all_widths.iter().enumerate() {
        let seg_width = if idx == all_widths.len() - 1 {
            // last segment (free) takes remaining space
            bar_width.saturating_sub(drawn)
        } else {
            ((*tokens as f64 / context_window as f64) * bar_width as f64).round() as usize
        };
        if seg_width == 0 {
            continue;
        }
        let bar = "█".repeat(seg_width.min(bar_width.saturating_sub(drawn)));
        spans.push(Span::styled(bar, Style::default().fg(*color)));
        drawn += seg_width;
    }

    let bar_line = Line::from(spans);
    lines.push(bar_line);

    // Legend line.
    let legend_items: Vec<String> = segments
        .iter()
        .map(|s| format!("{} {}", s.label, format_token_count(s.tokens)))
        .collect();
    let legend = legend_items.join("  ");

    let remaining_str = snap
        .remaining_input_budget
        .map(|r| format!("  free {}", format_token_count(r as usize)))
        .unwrap_or_default();

    lines.push(Line::from(Span::styled(
        format!("{}{}", legend, remaining_str),
        Style::default().fg(TEXT_MUTED),
    )));

    lines.push(Line::from(Span::styled(
        format!(
            "ctx {}  in/out {}/{}  cache hit {}  miss {}",
            format_token_count(context_window),
            format_token_count(snap.total_input_tokens as usize),
            format_token_count(snap.total_output_tokens as usize),
            format_token_count(snap.total_cache_hit_tokens as usize),
            format_token_count(snap.total_cache_miss_tokens as usize),
        ),
        Style::default().fg(TEXT_MUTED),
    )));
}

fn push_child_sessions(lines: &mut Vec<Line<'static>>, app: &TuiApp) {
    // Child / sub-agent sessions if any.
    let child_count = app
        .snapshot
        .pending_interactions
        .len();
    if child_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("sub-agents  {child_count} active"),
            Style::default()
                .fg(INTERACTION_SUB_AGENT)
                .add_modifier(Modifier::BOLD),
        )));
    }
}

fn push_context_summary(lines: &mut Vec<Line<'static>>, app: &TuiApp) {
    lines.push(Line::from(Span::styled(
        format!("history  {} turns", app.snapshot.history_len),
        Style::default().fg(TEXT_SECONDARY),
    )));

    if app.snapshot.compaction_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("compacted  {} times", app.snapshot.compaction_count),
            Style::default().fg(TEXT_MUTED),
        )));
    }
}

fn version_footer_line() -> Span<'static> {
    let version = option_env!("CARGO_PKG_VERSION").unwrap_or("dev");
    Span::styled(format!("rara v{version}"), Style::default().fg(TEXT_MUTED))
}

/// Format a token count for human-readable display.
fn format_token_count(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
