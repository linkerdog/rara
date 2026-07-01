// Bottom pane composer — input rendering, text wrapping, placeholder hints.
use std::cell::RefCell;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::super::custom_terminal::Frame;
use super::super::super::interaction_text::pending_interaction_hint_text;
use super::super::super::queued_input::{pending_follow_up_hint, queued_follow_up_hint};
use super::super::super::state::char_offset_to_byte_index;
use super::super::super::state::{ActivePendingInteractionKind, GoalStatus, TaskKind, TuiApp};
use super::bottom_pane_style;
use crate::tui::theme::{TEXT_ACCENT, TEXT_MUTED, TEXT_SECONDARY};

const COMPOSER_TAB_WIDTH: usize = 4;

pub(super) fn render_composer(f: &mut Frame, app: &mut TuiApp, area: Rect) -> Option<(u16, u16)> {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .split(area);
    let hide_input_for_approval = app
        .active_pending_interaction()
        .is_some_and(|p| p.kind != ActivePendingInteractionKind::RequestInput);

    let composer_lines = if hide_input_for_approval {
        // Show a single-line status when approval dock is active above.
        let pending = app.active_pending_interaction().unwrap();
        vec![Line::from(vec![Span::styled(
            format!(
                "› {} — use ↑↓ or keys to respond",
                super::super::super::interaction_text::pending_interaction_card_title(pending.kind),
            ),
            Style::default()
                .fg(TEXT_MUTED)
                .add_modifier(Modifier::ITALIC),
        )])]
    } else if app.bottom_pane.input.is_empty() {
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
        let rows = wrapped_text_rows(
            app.bottom_pane.input.as_str(),
            chunks[0].width,
            Some("› "),
            Some("  "),
        );
        let cursor_off = app.composer_cursor_offset();
        let cursor_row = find_cursor_row_in_wrapped(
            app.bottom_pane.input.as_str(),
            cursor_off,
            chunks[0].width,
            Some("› "),
            Some("  "),
        );
        app.maintain_composer_scroll(
            area.width,
            area.height.saturating_sub(1),
            cursor_row,
            rows.len(),
        );
        rows.into_iter()
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
            .wrap(Wrap { trim: false })
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
    if hide_input_for_approval {
        None
    } else {
        Some(composer_cursor_position(
            app.bottom_pane.input.as_str(),
            app.composer_cursor_offset(),
            chunks[0],
            app.bottom_pane.composer_scroll,
        ))
    }
}

pub(super) fn composer_hint(app: &TuiApp) -> Line<'static> {
    let text: &'static str = if matches!(
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
        if app
            .bottom_pane
            .running_task
            .as_ref()
            .is_some_and(|task| matches!(task.kind, TaskKind::Query))
        {
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

pub(super) fn composer_cursor_position(
    input: &str,
    cursor_offset: usize,
    area: Rect,
    scroll: usize,
) -> (u16, u16) {
    let (x, y) = wrapped_text_cursor_position(input, cursor_offset, area, Some("› "), Some("  "));
    let adjusted_y = y.saturating_sub(scroll as u16);
    (x, adjusted_y)
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
    let content = if app.bottom_pane.input.is_empty() {
        "Ask about the repo, request a code change, or type /help to browse commands.".to_string()
    } else {
        app.bottom_pane.input.clone()
    };

    wrapped_text_row_count(&content, width, Some("› "), None)
}

pub(crate) fn editor_cursor_position(input: &str, cursor_offset: usize, area: Rect) -> (u16, u16) {
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
    let cursor_y = area.y.saturating_add(row_index as u16);
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

pub(super) fn wrapped_text_rows(
    input: &str,
    width: u16,
    initial_indent: Option<&str>,
    subsequent_indent: Option<&str>,
) -> Vec<String> {
    // Simple cache: a single-entry cache is effective because the
    // same (input, width) pair is queried every frame for render,
    // cursor, and scroll — so the second and third calls are free.
    thread_local! {
        static CACHE: RefCell<Option<(String, u16, Vec<String>)>> = const { RefCell::new(None) };
    }
    CACHE.with(|cell| {
        if let Some((ref cached_input, w, ref rows)) = *cell.borrow()
            && cached_input == input
            && w == width
        {
            return rows.clone();
        }
        let rows = wrapped_text_rows_uncached(input, width, initial_indent, subsequent_indent);
        cell.replace(Some((input.to_string(), width, rows.clone())));
        rows
    })
}

fn wrapped_text_rows_uncached(
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

    {
        let mut lines = input.split('\n');
        // First logical line keeps the caller-supplied initial indent.
        if let Some(first) = lines.next() {
            wrapped_rows.extend(wrap_logical_line_preserving_whitespace(
                first,
                width,
                initial_indent,
                subsequent_indent,
            ));
        }
        // Lines after embedded newlines use the subsequent indent.
        for logical_line in lines {
            wrapped_rows.extend(wrap_logical_line_preserving_whitespace(
                logical_line,
                width,
                subsequent_indent,
                subsequent_indent,
            ));
        }
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

/// Find which wrapped row the cursor falls on by wrapping only the
/// cursor prefix substring — matching the approach in
/// `wrapped_text_cursor_position`.
pub(super) fn find_cursor_row_in_wrapped(
    input: &str,
    cursor_char_offset: usize,
    width: u16,
    initial_indent: Option<&str>,
    subsequent_indent: Option<&str>,
) -> usize {
    let cursor_prefix_end = char_offset_to_byte_index(input, cursor_char_offset);
    let cursor_prefix = &input[..cursor_prefix_end];
    let wrapped = wrapped_text_rows(cursor_prefix, width, initial_indent, subsequent_indent);
    wrapped.len().saturating_sub(1)
}

#[cfg(test)]
#[path = "../bottom_pane_tests.rs"]
mod bottom_pane_tests;
