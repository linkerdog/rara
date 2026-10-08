// Bottom pane composer — input rendering, text wrapping, placeholder hints.
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use super::super::super::custom_terminal::Frame;
use super::super::super::interaction_text::pending_interaction_hint_text;
use super::super::super::queued_input::{pending_follow_up_hint, queued_follow_up_hint};
use super::super::super::state::{ActivePendingInteractionKind, GoalStatus, TaskKind, TuiApp};
use super::bottom_pane_style;
use crate::tui::composer_text::{
    COMPOSER_INITIAL_INDENT, COMPOSER_SUBSEQUENT_INDENT, WrapConfig, clipped_cursor_column,
    expand_tabs, wrapped_composer, wrapped_text,
};
use crate::tui::theme::{TEXT_ACCENT, TEXT_MUTED, TEXT_SECONDARY};

const COMPOSER_PLACEHOLDER: &str =
    "Ask about the repo, request a code change, or type /help to browse commands.";

pub(super) fn render_composer(f: &mut Frame, app: &mut TuiApp, area: Rect) -> Option<(u16, u16)> {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .split(area);
    let pending_approval = app
        .active_pending_interaction()
        .filter(|p| p.kind != ActivePendingInteractionKind::RequestInput);
    let hide_input_for_approval = pending_approval.is_some();

    let composer_lines = if let Some(pending) = pending_approval {
        // Show a single-line status when approval dock is active above.
        vec![Line::from(vec![Span::styled(
            format!(
                "{COMPOSER_INITIAL_INDENT}{} — use ↑↓ or keys to respond",
                super::super::super::interaction_text::pending_interaction_card_title(pending.kind),
            ),
            Style::default()
                .fg(TEXT_MUTED)
                .add_modifier(Modifier::ITALIC),
        )])]
    } else {
        let is_placeholder = app.bottom_pane.input.is_empty();
        let content = if is_placeholder {
            COMPOSER_PLACEHOLDER
        } else {
            app.bottom_pane.input.as_str()
        };
        let layout = if is_placeholder {
            wrapped_text(content, WrapConfig::composer(chunks[0].width))
        } else {
            wrapped_composer(&app.bottom_pane, WrapConfig::composer(chunks[0].width))
        };
        let cursor_row = layout.cursor_position(app.composer_cursor_offset()).row;
        if is_placeholder {
            app.bottom_pane.composer_scroll = 0;
        } else {
            app.maintain_composer_scroll(
                area.width,
                area.height.saturating_sub(1),
                cursor_row,
                layout.rows().len(),
            );
        }
        layout
            .rows()
            .iter()
            .map(|row| {
                let mut spans = Vec::new();
                let (prefix, remainder) =
                    if let Some(rest) = row.strip_prefix(COMPOSER_INITIAL_INDENT) {
                        (COMPOSER_INITIAL_INDENT, rest)
                    } else if let Some(rest) = row.strip_prefix(COMPOSER_SUBSEQUENT_INDENT) {
                        (COMPOSER_SUBSEQUENT_INDENT, rest)
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
                if is_placeholder {
                    let style = Style::default().fg(TEXT_SECONDARY);
                    if let Some((before, after)) = remainder.split_once("/help") {
                        spans.push(Span::styled(before.to_string(), style));
                        spans.push(Span::styled(
                            "/help",
                            Style::default()
                                .fg(TEXT_ACCENT)
                                .add_modifier(Modifier::BOLD),
                        ));
                        spans.push(Span::styled(after.to_string(), style));
                    } else {
                        spans.push(Span::styled(remainder.to_string(), style));
                    }
                } else {
                    spans.push(Span::raw(expand_tabs(remainder)));
                }
                Line::from(spans)
            })
            .collect::<Vec<_>>()
    };
    f.render_widget(
        Paragraph::new(composer_lines)
            .block(Block::default())
            .style(bottom_pane_style())
            .scroll((app.bottom_pane.composer_scroll as u16, 0)),
        chunks[0],
    );
    let hint = composer_hint_line(app);
    f.render_widget(
        Paragraph::new(hint)
            .style(bottom_pane_style())
            .alignment(Alignment::Left),
        chunks[1],
    );
    if hide_input_for_approval || chunks[0].is_empty() {
        None
    } else {
        Some(composer_cursor_position(
            app,
            chunks[0],
            app.bottom_pane.composer_scroll,
        ))
    }
}

pub(super) fn composer_hint(app: &TuiApp) -> Line<'static> {
    let text =
        if matches!(
            app.overlay,
            Some(super::super::super::state::Overlay::CommandPalette)
        ) {
            ""
        } else if app.bottom_pane.input.trim_start().starts_with('/') {
            "slash command  Enter run  Esc close"
        } else if let Some(pending) = app.active_pending_interaction() {
            pending_interaction_hint_text(pending.kind)
        } else if app.has_pending_follow_up_messages() {
            pending_follow_up_hint()
        } else if app.has_queued_follow_up_messages() {
            queued_follow_up_hint()
        } else if app.is_busy() {
            if app.bottom_pane.running_task.as_ref().is_some_and(|task| {
                matches!(task.kind, TaskKind::Query | TaskKind::ReviewPreparation)
            }) {
                "Enter queue  Esc/Ctrl+C cancel"
            } else {
                "Enter queue"
            }
        } else if app.has_pending_planning_suggestion() {
            "planning suggested  1 enter planning mode  2 continue in execute mode"
        } else if app.agent_execution_mode_label() == "plan" {
            "planning mode  read-only planning; approve to execute"
        } else {
            ""
        };

    if text.is_empty() {
        return Line::default();
    }
    parse_hint_with_keys(text)
}

/// Split hint text on whitespace-delimited single-digit numbers like " 1 " and
/// highlight the digits with a keycap-like accent.
pub(super) fn parse_hint_with_keys(text: &'static str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text;
    while let Some(pos) = remaining.find(|c: char| c.is_ascii_digit()) {
        if pos > 0 {
            spans.push(Span::styled(
                &remaining[..pos],
                Style::default().fg(TEXT_MUTED),
            ));
        }
        let digit_end = remaining[pos..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(remaining.len(), |d| pos + d);
        spans.push(Span::styled(
            &remaining[pos..digit_end],
            Style::default().fg(TEXT_ACCENT),
        ));
        remaining = &remaining[digit_end..];
    }
    if !remaining.is_empty() {
        spans.push(Span::styled(remaining, Style::default().fg(TEXT_MUTED)));
    }
    Line::from(spans)
}

pub(super) fn composer_hint_line(app: &TuiApp) -> Line<'static> {
    composer_hint(app)
}

pub(super) fn composer_cursor_position(app: &TuiApp, area: Rect, scroll: usize) -> (u16, u16) {
    let layout = wrapped_composer(&app.bottom_pane, WrapConfig::composer(area.width));
    let position = layout.cursor_position(app.composer_cursor_offset());
    (
        area.x.saturating_add(position.column as u16),
        area.y.saturating_add(
            position
                .row
                .saturating_sub(scroll)
                .min(area.height.saturating_sub(1) as usize) as u16,
        ),
    )
}

pub(crate) fn desired_composer_height(app: &TuiApp, width: u16, rows: u16) -> u16 {
    let available_width = width.max(1);
    let content_rows = composer_content_line_count(app, available_width);
    // Cap at 40% of terminal height (Codex style) so the transcript
    // stays visible above a growing input.
    // `.max(3)` keeps `clamp` safe even when rows < 7.
    let max_height = ((rows as f64 * 0.4).ceil() as u16).clamp(3, rows.saturating_sub(4).max(3));
    content_rows.clamp(3, max_height)
}

pub(super) fn composer_content_line_count(app: &TuiApp, width: u16) -> u16 {
    let layout = if app.bottom_pane.input.is_empty() {
        wrapped_text(COMPOSER_PLACEHOLDER, WrapConfig::composer(width))
    } else {
        wrapped_composer(&app.bottom_pane, WrapConfig::composer(width))
    };
    u16::try_from(layout.rows().len()).unwrap_or(u16::MAX)
}

pub(crate) fn editor_cursor_position(input: &str, cursor_offset: usize, area: Rect) -> (u16, u16) {
    let inner = inner_rect(area);
    let column = clipped_cursor_column(input, cursor_offset, inner.width) as u16;
    (inner.x.saturating_add(column), inner.y)
}

fn inner_rect(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

#[cfg(test)]
pub(super) fn wrapped_text_cursor_position(
    input: &str,
    cursor_offset: usize,
    area: Rect,
    initial_indent: Option<&str>,
    subsequent_indent: Option<&str>,
) -> (u16, u16) {
    if area.width == 0 || area.height == 0 {
        return (area.x, area.y);
    }

    let layout = wrapped_text(
        input,
        WrapConfig {
            width: area.width,
            initial_indent: initial_indent.unwrap_or(""),
            subsequent_indent: subsequent_indent.unwrap_or(""),
        },
    );
    let position = layout.cursor_position(cursor_offset);
    let row = u16::try_from(position.row).unwrap_or(u16::MAX);
    let column = u16::try_from(position.column).unwrap_or(u16::MAX);
    (area.x.saturating_add(column), area.y.saturating_add(row))
}

pub(super) fn wrapped_text_rows(
    input: &str,
    width: u16,
    initial_indent: Option<&str>,
    subsequent_indent: Option<&str>,
) -> Vec<String> {
    wrapped_text(
        input,
        WrapConfig {
            width,
            initial_indent: initial_indent.unwrap_or(""),
            subsequent_indent: subsequent_indent.unwrap_or(""),
        },
    )
    .rows()
    .to_vec()
}

#[cfg(test)]
#[path = "../bottom_pane_tests.rs"]
mod bottom_pane_tests;
