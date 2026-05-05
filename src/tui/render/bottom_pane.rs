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
use super::super::state::{ActivePendingInteractionKind, TaskKind, TuiApp};
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
    let composer_rows = desired_composer_height(app, width, rows);
    let total = composer_rows.saturating_add(2);
    let max = rows.max(1);
    let min = 5.min(max);
    total.clamp(min, max)
}

pub(super) fn render_bottom_pane(f: &mut Frame, app: &TuiApp, area: Rect) -> Option<(u16, u16)> {
    f.render_widget(Block::default().style(bottom_pane_style()), area);
    let composer_height = area.height.saturating_sub(2).max(3);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);
    render_activity_bar(f, app, chunks[0]);
    let cursor = render_composer(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);
    cursor
}

fn render_activity_bar(f: &mut Frame, app: &TuiApp, area: Rect) {
    let (label, color, detail) = activity_status_line(app);
    let mut spans = vec![Span::styled(
        animated_activity_label(app, label),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )];
    let label_already_reflects_planning = matches!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(
            ActivePendingInteractionKind::PlanApproval
                | ActivePendingInteractionKind::PlanningQuestion
        )
    ) || matches!(label, "Planning");

    if app.agent_execution_mode_label() == "plan" && !label_already_reflects_planning {
        spans.push(Span::raw("  "));
        spans.push(badge("mode", "plan", TEXT_ACCENT));
    }
    if app.permission_mode_label() != "auto" {
        spans.push(Span::raw("  "));
        spans.push(badge("perm", app.permission_mode_label(), STATUS_INFO));
    }
    if !detail.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(detail, Style::default().fg(TEXT_SECONDARY)));
    }
    let status = Paragraph::new(Line::from(spans)).style(bottom_pane_style());
    f.render_widget(status, area);
}

fn activity_status_line(app: &TuiApp) -> (&'static str, Color, String) {
    if matches!(
        app.runtime_phase,
        super::super::state::RuntimePhase::RebuildingBackend
    ) {
        return (
            "Downloading",
            STATUS_INFO,
            app.runtime_phase_detail
                .as_deref()
                .unwrap_or("preparing backend")
                .to_string(),
        );
    }

    if let Some(pending) = app.active_pending_interaction() {
        let (label, color) = match pending.kind {
            ActivePendingInteractionKind::PlanApproval => ("Plan Approval", TEXT_ACCENT),
            ActivePendingInteractionKind::ShellApproval => ("Shell Approval", STATUS_WARNING),
            ActivePendingInteractionKind::PlanningQuestion => ("Planning Question", TEXT_ACCENT),
            ActivePendingInteractionKind::ExplorationQuestion => {
                ("Exploration Question", STATUS_WARNING)
            }
            ActivePendingInteractionKind::SubAgentQuestion => {
                ("Sub-agent Question", INTERACTION_SUB_AGENT)
            }
            ActivePendingInteractionKind::RequestInput => ("Request Input", STATUS_SUCCESS),
        };
        let detail = match pending.kind {
            ActivePendingInteractionKind::PlanApproval => {
                "choose whether to start implementation or continue planning".to_string()
            }
            ActivePendingInteractionKind::ShellApproval => app
                .pending_command_approval()
                .map(|interaction| interaction.summary.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "review the pending shell command".to_string()),
            ActivePendingInteractionKind::PlanningQuestion
            | ActivePendingInteractionKind::ExplorationQuestion
            | ActivePendingInteractionKind::SubAgentQuestion
            | ActivePendingInteractionKind::RequestInput => app
                .pending_request_input()
                .map(|interaction| interaction.title.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "answer the pending question".to_string()),
        };
        return (label, color, detail);
    }

    if app.has_pending_planning_suggestion() {
        return (
            "Planning Suggested",
            TEXT_ACCENT,
            "enter planning mode first or continue in execute mode".to_string(),
        );
    }

    if app.is_busy() {
        let elapsed = app
            .running_elapsed()
            .map(|d| {
                let secs = d.as_secs();
                if secs < 60 {
                    format!("{}s", secs)
                } else {
                    let mins = secs / 60;
                    let remain_secs = secs % 60;
                    format!("{}m {}s", mins, remain_secs)
                }
            })
            .unwrap_or_else(|| "…".to_string());
        return (
            "Working",
            STATUS_WARNING,
            format!("({} • esc to interrupt)", elapsed),
        );
    }

    if app.agent_execution_mode_label() == "plan" {
        return (
            "Planning",
            TEXT_ACCENT,
            "read-only planning; approve to execute".to_string(),
        );
    }

    if let Some(warning) = app
        .notice
        .as_deref()
        .filter(|value| value.starts_with("Warning:"))
    {
        return ("Warning", STATUS_WARNING, warning.to_string());
    }

    (
        "Ready",
        STATUS_READY,
        app.notice
            .as_deref()
            .filter(|notice| !matches!(*notice, "Prompt finished." | "Planning finished."))
            .unwrap_or("waiting for input")
            .to_string(),
    )
}

fn animated_activity_label(app: &TuiApp, label: &str) -> String {
    if label.is_empty() {
        return String::new();
    }
    let Some(task) = app.running_task.as_ref() else {
        return label.to_string();
    };
    if !matches!(task.kind, TaskKind::Query | TaskKind::Rebuild) {
        return label.to_string();
    }

    let dots = match (task.started_at.elapsed().as_millis() / 450) % 3 {
        0 => ".",
        1 => "..",
        _ => "...",
    };
    format!("{label}{dots}")
}

fn render_composer(f: &mut Frame, app: &TuiApp, area: Rect) -> Option<(u16, u16)> {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .split(area);
    let composer_lines = if app.input.is_empty() {
        vec![Line::from(vec![
            Span::styled(
                "› ",
                Style::default()
                    .fg(TEXT_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Ask about the repo, request a code change, or type ",
                Style::default().fg(TEXT_SECONDARY),
            ),
            Span::styled(
                "/help",
                Style::default()
                    .fg(TEXT_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to browse commands.", Style::default().fg(TEXT_SECONDARY)),
        ])]
    } else {
        wrapped_text_rows(app.input.as_str(), chunks[0].width, Some("› "), Some("  "))
            .into_iter()
            .map(|row| {
                let mut spans = Vec::new();
                let (prefix, remainder) = if let Some(rest) = row.strip_prefix("› ") {
                    ("› ", rest)
                } else if let Some(rest) = row.strip_prefix("  ") {
                    ("  ", rest)
                } else {
                    ("", row.as_str())
                };

                if !prefix.is_empty() {
                    spans.push(Span::styled(
                        prefix.to_string(),
                        Style::default()
                            .fg(TEXT_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                spans.push(Span::raw(expand_composer_display_text(remainder)));
                Line::from(spans)
            })
            .collect::<Vec<_>>()
    };
    f.render_widget(
        Paragraph::new(composer_lines)
            .block(Block::default())
            .style(bottom_pane_style())
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    let hint = composer_hint_line(app);
    f.render_widget(
        Paragraph::new(hint)
            .style(bottom_pane_style())
            .alignment(Alignment::Left),
        chunks[1],
    );
    Some(composer_cursor_position(
        app.input.as_str(),
        app.composer_cursor_offset(),
        chunks[0],
    ))
}

fn composer_hint(app: &TuiApp) -> &'static str {
    if matches!(
        app.overlay,
        Some(super::super::state::Overlay::CommandPalette)
    ) {
        ""
    } else if app.input.trim_start().starts_with('/') {
        "slash command  Enter run  Esc close"
    } else if let Some(pending) = app.active_pending_interaction() {
        pending_interaction_hint_text(pending.kind)
    } else if app.has_pending_follow_up_messages() {
        pending_follow_up_hint()
    } else if app.has_queued_follow_up_messages() {
        queued_follow_up_hint()
    } else if app.is_busy() {
        if app
            .running_task
            .as_ref()
            .is_some_and(|task| matches!(task.kind, TaskKind::Query))
        {
            "Enter queue  Esc cancel"
        } else {
            "Enter queue"
        }
    } else if app.has_pending_planning_suggestion() {
        "planning suggested  1 enter planning mode  2 continue in execute mode"
    } else if app.agent_execution_mode_label() == "plan" {
        "planning mode  read-only planning; approve to execute"
    } else {
        ""
    }
}

fn composer_hint_line(app: &TuiApp) -> Line<'static> {
    let hint = composer_hint(app);
    let mut spans = Vec::new();
    if !hint.is_empty() {
        spans.push(Span::styled(hint, Style::default().fg(TEXT_MUTED)));
    }
    Line::from(spans)
}

fn render_footer(f: &mut Frame, app: &TuiApp, area: Rect) {
    if matches!(
        app.overlay,
        Some(super::super::state::Overlay::CommandPalette)
    ) {
        f.render_widget(Paragraph::new("").style(bottom_pane_style()), area);
        return;
    }
    let summary = footer_summary_text(app);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            summary,
            Style::default().fg(TEXT_SECONDARY),
        )))
        .style(bottom_pane_style())
        .alignment(Alignment::Right),
        area,
    );
}

fn bottom_pane_style() -> Style {
    Style::default().bg(BOTTOM_PANE_BG)
}

fn footer_summary_text(app: &TuiApp) -> String {
    let prefix = app
        .repo_context_hint()
        .unwrap_or_default();

    let cache_summary = cache_hit_rate_label(
        app.snapshot.total_cache_hit_tokens,
        app.snapshot.total_cache_miss_tokens,
    )
    .map(|rate| format!("  cache_hit={rate}"))
    .unwrap_or_default();
    if shows_live_task_stats(app) {
        format!(
            "{}  tokens={} in / {} out{}",
            prefix,
            app.snapshot.total_input_tokens,
            app.snapshot.total_output_tokens,
            cache_summary
        )
    } else if app.snapshot.compaction_count > 0 {
        format!(
            "{}  compactions={}{}",
            prefix, app.snapshot.compaction_count, cache_summary
        )
    } else {
        format!("{prefix}{cache_summary}")
    }
}

fn shows_live_task_stats(app: &TuiApp) -> bool {
    app.is_busy()
        || matches!(
            app.runtime_phase,
            super::super::state::RuntimePhase::SendingPrompt
                | super::super::state::RuntimePhase::ProcessingResponse
                | super::super::state::RuntimePhase::RunningTool
        )
}

fn composer_cursor_position(input: &str, cursor_offset: usize, area: Rect) -> (u16, u16) {
    wrapped_text_cursor_position(input, cursor_offset, area, Some("› "), Some("  "))
}

fn desired_composer_height(app: &TuiApp, width: u16, rows: u16) -> u16 {
    let available_width = width.max(1);
    let content_rows = composer_content_line_count(app, available_width);
    let max_height = rows.saturating_sub(4).max(3);
    content_rows.clamp(3, max_height)
}

fn composer_content_line_count(app: &TuiApp, width: u16) -> u16 {
    let content = if app.input.is_empty() {
        "Ask about the repo, request a code change, or type /help to browse commands.".to_string()
    } else {
        app.input.clone()
    };

    wrapped_text_row_count(&content, width, Some("› "), None)
}

pub(super) fn editor_cursor_position(input: &str, cursor_offset: usize, area: Rect) -> (u16, u16) {
    wrapped_text_cursor_position(input, cursor_offset, inner_rect(area), None, None)
}

fn inner_rect(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

fn wrapped_text_cursor_position(
    input: &str,
    cursor_offset: usize,
    area: Rect,
    initial_indent: Option<&str>,
    subsequent_indent: Option<&str>,
) -> (u16, u16) {
    if area.width == 0 || area.height == 0 {
        return (area.x, area.y);
    }

    let initial_indent = initial_indent.unwrap_or("");
    let subsequent_indent = subsequent_indent.unwrap_or("");
    let cursor_prefix_end = char_offset_to_byte_index(input, cursor_offset);
    let cursor_prefix = &input[..cursor_prefix_end];
    let wrapped_rows = wrapped_text_rows(
        cursor_prefix,
        area.width,
        Some(initial_indent),
        Some(subsequent_indent),
    );

    let last_row = wrapped_rows
        .last()
        .cloned()
        .unwrap_or_else(|| initial_indent.to_string());
    let row_index = wrapped_rows.len().saturating_sub(1);
    let cursor_y = area
        .y
        .saturating_add(row_index.min(area.height.saturating_sub(1) as usize) as u16);
    let display_width = display_text_width(last_row.as_str()) as u16;
    let max_x_offset = area.width.saturating_sub(1);
    let cursor_x = area.x.saturating_add(display_width.min(max_x_offset));

    (cursor_x, cursor_y)
}

fn wrapped_text_row_count(
    input: &str,
    width: u16,
    initial_indent: Option<&str>,
    subsequent_indent: Option<&str>,
) -> u16 {
    wrapped_text_rows(input, width, initial_indent, subsequent_indent).len() as u16
}

fn expand_composer_display_text(text: &str) -> String {
    let mut expanded = String::new();
    for ch in text.chars() {
        match ch {
            '\t' => expanded.push_str(&" ".repeat(COMPOSER_TAB_WIDTH)),
            _ => expanded.push(ch),
        }
    }
    expanded
}

fn display_text_width(text: &str) -> usize {
    text.chars().map(display_char_width).sum()
}

fn wrapped_text_rows(
    input: &str,
    width: u16,
    initial_indent: Option<&str>,
    subsequent_indent: Option<&str>,
) -> Vec<String> {
    let width = width.max(1);
    let initial_indent = initial_indent.unwrap_or("");
    let subsequent_indent = subsequent_indent.unwrap_or("");
    let mut wrapped_rows = Vec::new();

    if input.is_empty() {
        wrapped_rows.push(initial_indent.to_string());
        return wrapped_rows;
    }

    for logical_line in input.split('\n') {
        wrapped_rows.extend(wrap_logical_line_preserving_whitespace(
            logical_line,
            width,
            initial_indent,
            subsequent_indent,
        ));
    }

    wrapped_rows
}

fn wrap_logical_line_preserving_whitespace(
    logical_line: &str,
    width: u16,
    initial_indent: &str,
    subsequent_indent: &str,
) -> Vec<String> {
    let max_width = width.max(1) as usize;
    let initial_width = UnicodeWidthStr::width(initial_indent);
    let subsequent_width = UnicodeWidthStr::width(subsequent_indent);
    let mut rows = Vec::new();
    let mut current = initial_indent.to_string();
    let mut current_width = initial_width.min(max_width);
    let mut current_prefix_width = initial_width.min(max_width);

    if logical_line.is_empty() {
        rows.push(current);
        return rows;
    }

    for ch in logical_line.chars() {
        let char_width = display_char_width(ch);
        let next_width = current_width.saturating_add(char_width);
        let can_wrap = current_width > current_prefix_width;
        if next_width > max_width && can_wrap {
            rows.push(current);
            current = subsequent_indent.to_string();
            current_prefix_width = subsequent_width.min(max_width);
            current_width = current_prefix_width;
        }
        current.push(ch);
        current_width = current_width.saturating_add(char_width);
    }

    rows.push(current);
    rows
}

fn display_char_width(ch: char) -> usize {
    match ch {
        '\t' => COMPOSER_TAB_WIDTH,
        _ => UnicodeWidthChar::width(ch).unwrap_or(0),
    }
}

#[cfg(test)]
#[path = "bottom_pane_tests.rs"]
mod tests;
