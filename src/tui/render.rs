mod bottom_pane;
pub(crate) mod cells;
pub(crate) mod diff;
mod helpers;
mod history_pipeline;
mod overlay;
#[cfg(test)]
mod tests;
mod viewport;

use std::path::Path;

pub(crate) use helpers::*;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

pub(crate) use self::bottom_pane::desired_viewport_height;
use self::bottom_pane::{desired_bottom_pane_height, render_bottom_pane};
pub(crate) use self::cells::{ActiveCell, HistoryCell};
use self::cells::{ActiveTurnCell, CommittedTurnCell, StartupCardCell};
use self::overlay::render_overlay;
use self::viewport::TranscriptViewport;
use super::custom_terminal::Frame;
use super::line_utils::prefix_lines;
use super::state::{TranscriptEntry, TuiApp};
use super::tool_text::{compact_delegate_rest, compact_instruction};
use crate::tui::sub_agent_display::SubAgentKind;
use crate::tui::theme::*;

pub fn render(f: &mut Frame, app: &TuiApp) {
    let bottom_pane_height = desired_bottom_pane_height(app, f.area().width, f.area().height);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(bottom_pane_height)])
        .split(f.area());

    let transcript_area = render_startup_header(f, app, layout[0]);
    render_transcript(f, app, transcript_area);
    let mut cursor = render_bottom_pane(f, app, layout[1]);

    if let Some(overlay) = app.overlay {
        cursor = render_overlay(f, app, overlay, layout[1]).or(cursor);
    }

    if let Some((x, y)) = cursor {
        f.set_cursor_position((x, y));
    }
}

fn render_startup_header(f: &mut Frame, app: &TuiApp, area: Rect) -> Rect {
    if !shows_startup_header(app) {
        return area;
    }

    let lines = startup_card_lines(app, area.width);
    if lines.is_empty() {
        return area;
    }

    let header_height = (lines.len() as u16).min(area.height.saturating_sub(1).max(1));
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Fill(1)])
        .split(area);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), layout[0]);
    layout[1]
}

fn shows_startup_header(app: &TuiApp) -> bool {
    !app.has_any_transcript()
        && app.active_turn.entries.is_empty()
        && !app.is_busy()
        && !app.has_pending_planning_suggestion()
}

fn render_transcript(f: &mut Frame, app: &TuiApp, area: Rect) {
    let viewport = transcript_viewport(app, area.width, area.height);
    if !app.has_any_transcript() && viewport.lines.is_empty() {
        let lines = vec![
            Line::from(Span::styled(
                "Ready.",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("Use the input bar below to start a task or run a local command."),
            Line::from(""),
            Line::from(Span::styled(
                "Start with:",
                Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("  /help    browse built-in commands and runtime hints"),
            Line::from("  /model   choose provider first, then switch models"),
            Line::from("  /status  inspect runtime, tokens, cache, and session"),
            Line::from("  /quit    leave the TUI and restore the terminal"),
            Line::from(""),
            Line::from(Span::styled(
                "Prompt ideas:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("  Explain this repository structure."),
            Line::from("  Find the main agent loop and summarize it."),
        ];
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        return;
    }

    viewport.render(f, area);
}

pub(crate) fn transcript_viewport(
    app: &TuiApp,
    width: u16,
    viewport_height: u16,
) -> TranscriptViewport {
    let lines = renderable_transcript_lines(app, width);
    let visual_row_count = transcript_visual_row_count(&lines, width);
    let effective_height = viewport_height.saturating_sub(1).max(1);
    let scroll_offset = transcript_scroll_offset(app, effective_height, visual_row_count);
    TranscriptViewport::new(lines, scroll_offset)
}

fn renderable_transcript_lines(app: &TuiApp, width: u16) -> Vec<Line<'static>> {
    let mut lines = committed_transcript_lines(app, width);

    let mut active_lines = active_turn_cell(app).display_lines(width);
    if !active_lines.is_empty() {
        if !lines.is_empty() {
            lines.push(turn_divider_line(width));
        }
        lines.append(&mut active_lines);
    }

    lines
}

fn committed_transcript_lines(app: &TuiApp, width: u16) -> Vec<Line<'static>> {
    {
        let cache = app.committed_render_cache.borrow();
        if cache.generation == app.committed_render_generation && cache.width == width {
            return cache.lines.clone();
        }
    }

    let cwd = (!app.snapshot.cwd.is_empty()).then(|| Path::new(app.snapshot.cwd.as_str()));
    let mut lines = Vec::new();
    for turn in &app.committed_turns {
        let mut turn_lines = committed_turn_lines(turn.entries.as_slice(), cwd, width);
        if turn_lines.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(turn_divider_line(width));
        }
        lines.append(&mut turn_lines);
    }

    let mut cache = app.committed_render_cache.borrow_mut();
    cache.generation = app.committed_render_generation;
    cache.width = width;
    cache.lines = lines.clone();
    lines
}

fn transcript_scroll_offset(
    app: &TuiApp,
    viewport_height: u16,
    transcript_line_count: usize,
) -> u16 {
    let max_offset = transcript_line_count.saturating_sub(viewport_height as usize);
    let top_offset = max_offset.saturating_sub(app.transcript_scroll);
    top_offset.min(u16::MAX as usize) as u16
}

fn transcript_visual_row_count(lines: &[Line<'static>], width: u16) -> usize {
    super::layout_utils::total_visual_rows(lines, width)
}

fn turn_divider_line(width: u16) -> Line<'static> {
    let divider_width = usize::from(width.max(8));
    Line::from(Span::styled(
        "─".repeat(divider_width),
        Style::default().fg(Color::DarkGray),
    ))
}

pub fn committed_turn_cell<'a>(
    entries: &'a [TranscriptEntry],
    cwd: Option<&'a Path>,
) -> CommittedTurnCell<'a> {
    CommittedTurnCell::new(entries, cwd)
}

pub(crate) fn committed_turn_lines(
    entries: &[TranscriptEntry],
    cwd: Option<&Path>,
    width: u16,
) -> Vec<Line<'static>> {
    committed_turn_cell(entries, cwd).display_lines(width)
}

pub fn active_turn_cell<'a>(app: &'a TuiApp) -> ActiveTurnCell<'a> {
    let cwd = (!app.snapshot.cwd.is_empty()).then(|| Path::new(app.snapshot.cwd.as_str()));
    ActiveTurnCell::new(app, cwd)
}

pub fn startup_card_cell(app: &TuiApp) -> StartupCardCell {
    StartupCardCell::new(
        app.current_model_label().to_string(),
        display_directory_for_startup(app),
    )
}

pub(crate) fn startup_card_lines(app: &TuiApp, width: u16) -> Vec<Line<'static>> {
    startup_card_cell(app).display_lines(width)
}

fn current_turn_exploration_summary(
    app: &TuiApp,
    current_turn: &[&TranscriptEntry],
    prefer_live_label: bool,
) -> Option<String> {
    current_turn_exploration_summary_from_entries(
        current_turn,
        app.is_busy() && prefer_live_label,
        app.runtime_phase_detail.as_deref(),
    )
}

pub(crate) fn current_turn_exploration_summary_from_entries(
    current_turn: &[&TranscriptEntry],
    _show_live_detail: bool,
    _live_detail: Option<&str>,
) -> Option<String> {
    let mut actions = Vec::new();
    for entry in current_turn {
        use crate::tui::message_role::MessageRole;
        if MessageRole::try_from_str(&entry.role) != Some(MessageRole::Tool) {
            continue;
        }
        if let Some(action) = exploration_action_label(&entry.message) {
            if !actions.iter().any(|existing| existing == &action) {
                actions.push(action);
            }
        }
    }
    if actions.is_empty() {
        return None;
    }
    Some(compact_summary_lines(
        actions.as_slice(),
        4,
        "more file(s) inspected",
    ))
}

pub(crate) fn current_turn_tool_summary(
    current_turn: &[&TranscriptEntry],
    _show_live_detail: bool,
    _live_detail: Option<&str>,
) -> Option<String> {
    const RESULT_LINE_LIMIT: usize = 10;

    let mut lines = Vec::new();
    let mut pending_tool = false;
    for entry in current_turn {
        use crate::tui::message_role::MessageRole;
        match MessageRole::try_from_str(&entry.role) {
            Some(MessageRole::Tool) => {
                if let Some(action) = tool_action_label(&entry.message) {
                    lines.push(format!("└ {action}"));
                    pending_tool = true;
                } else {
                    pending_tool = false;
                }
            }
            Some(MessageRole::ToolResult) | Some(MessageRole::ToolError) if pending_tool => {
                lines.extend(
                    tool_result_summary_lines(&entry.message, RESULT_LINE_LIMIT)
                        .into_iter()
                        .map(|line| format!("  {line}")),
                );
                pending_tool = false;
            }
            _ => {}
        }
    }

    if lines.is_empty() {
        return None;
    }

    Some(lines.join("\n"))
}

fn tool_result_summary_lines(message: &str, max_lines: usize) -> Vec<String> {
    let result_lines = message
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .filter(|line| line.trim() != "preview available")
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if result_lines.is_empty() {
        return Vec::new();
    }

    let hidden_count = truncated_line_count(result_lines.len(), max_lines);
    let (head, tail) = head_tail_line_window(result_lines.as_slice(), max_lines);
    let mut rendered = head.to_vec();
    if hidden_count > 0 {
        rendered.push(format!("... {hidden_count} more line(s)"));
    }
    rendered.extend(tail.iter().cloned());
    rendered
}

pub(crate) fn compact_summary_lines(
    items: &[String],
    max_visible: usize,
    more_label: &str,
) -> String {
    if items.is_empty() {
        return String::new();
    }

    let visible_count = items.len().min(max_visible);
    let hidden_count = items.len().saturating_sub(visible_count);
    let start = items.len().saturating_sub(visible_count);
    let mut lines = items[start..]
        .iter()
        .map(|item| format!("└ {item}"))
        .collect::<Vec<_>>();

    if hidden_count > 0 {
        lines.insert(0, format!("└ ... {hidden_count} {more_label}"));
    }

    lines.join("\n")
}

pub(crate) fn compact_progress_summary_lines(
    actions: &[String],
    notes: &[String],
    max_visible: usize,
    more_label: &str,
) -> String {
    let mut lines = Vec::new();

    if let Some(note) = notes
        .iter()
        .rev()
        .map(|note| note.trim())
        .find(|note| !note.is_empty())
        .map(ToString::to_string)
    {
        lines.push(format!("└ {note}"));
    }

    if actions.is_empty() {
        return lines.join("\n");
    }

    let visible_count = actions.len().min(max_visible);
    let hidden_count = actions.len().saturating_sub(visible_count);
    let start = actions.len().saturating_sub(visible_count);

    if hidden_count > 0 {
        lines.push(format!("└ ... {hidden_count} {more_label}"));
    }

    lines.extend(actions[start..].iter().map(|action| format!("└ {action}")));
    lines.join("\n")
}

pub(crate) fn compact_recent_first_summary_lines(
    items: &[String],
    max_visible: usize,
    more_label: &str,
) -> String {
    if items.is_empty() {
        return String::new();
    }

    let visible_count = items.len().min(max_visible);
    let hidden_count = items.len().saturating_sub(visible_count);
    let visible = items
        .iter()
        .rev()
        .take(visible_count)
        .cloned()
        .collect::<Vec<_>>();

    let mut lines = Vec::new();
    if let Some(current) = visible.first() {
        lines.push(format!("└ {current}"));
    }
    if hidden_count > 0 {
        lines.push(format!("└ ... {hidden_count} {more_label}"));
    }
    lines.extend(visible.iter().skip(1).map(|item| format!("└ {item}")));
    lines.join("\n")
}

pub(crate) fn compact_summary_text(summary: &str, max_visible: usize, more_label: &str) -> String {
    let items = summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.trim_start_matches("└")
                .trim_start_matches("•")
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if items.is_empty() {
        return summary.trim().to_string();
    }

    compact_summary_lines(items.as_slice(), max_visible, more_label)
}

fn truncated_line_count(total: usize, max_lines: usize) -> usize {
    total.saturating_sub(max_lines)
}

fn head_tail_line_window<T>(items: &[T], max_lines: usize) -> (&[T], &[T]) {
    if max_lines == usize::MAX || items.len() <= max_lines {
        return (items, &[]);
    }
    if max_lines <= 1 {
        return (&items[..1], &[]);
    }

    let tail_len = max_lines - 1;
    (&items[..1], &items[items.len() - tail_len..])
}

fn role_prefix_icon(role: &str) -> (&'static str, Color) {
    use crate::tui::message_role::MessageRole;
    match MessageRole::try_from_str(role) {
        Some(MessageRole::User) => ("You", ROLE_USER),
        Some(MessageRole::Agent) => ("Agent", ROLE_AGENT),
        Some(MessageRole::System) => ("System", ROLE_SYSTEM),
        Some(MessageRole::ToolResult) => ("✓", STATUS_SUCCESS),
        Some(MessageRole::ToolError) => ("✕", STATUS_ERROR),
        Some(MessageRole::ToolProgress) => ("…", STATUS_WARNING),
        Some(MessageRole::Tool) => ("⚙", TEXT_SECONDARY),
        Some(MessageRole::Exploring) => ("🔍", PHASE_EXPLORING),
        Some(MessageRole::Planning) => ("📋", PHASE_PLANNING),
        Some(MessageRole::Running) => ("▶", PHASE_RUNNING),
        Some(MessageRole::Todo) => ("☑", PHASE_PLANNING),
        _ => ("", TEXT_SECONDARY),
    }
}

pub(crate) fn prefixed_message_lines(
    role: &str,
    message: &str,
    max_lines: usize,
) -> Vec<Line<'static>> {
    use crate::tui::message_role::MessageRole;
    let role_kind = MessageRole::try_from_str(role);
    let role_kind = MessageRole::try_from_str(role);
    if role_kind == Some(MessageRole::User) {
        return user_message_lines(message, usize::MAX);
    }
    if role_kind == Some(MessageRole::Agent) {
        return agent_message_lines(message, usize::MAX);
    }
    if role_kind == Some(MessageRole::System) {
        return system_message_lines(message, usize::MAX);
    }
    let (icon, color) = role_prefix_icon(role);
    let label = if icon.is_empty() {
        format!("{}:", role)
    } else {
        icon.to_string()
    };
    let message_lines = message.lines().collect::<Vec<_>>();
    if message_lines.is_empty() {
        return vec![Line::from(vec![Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )])];
    }

    let mut lines = Vec::new();
    let hidden_count = truncated_line_count(message_lines.len(), max_lines);
    let (head, tail) = head_tail_line_window(message_lines.as_slice(), max_lines);
    if let Some(first) = head.first() {
        let mut spans = vec![Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )];
        spans.push(Span::raw(format!(" {first}")));
        lines.push(Line::from(spans));
    }
    if hidden_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ... {} more line(s)", hidden_count),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
    lines.extend(tail.iter().map(|line| Line::from(format!("  {line}"))));
    lines
}
fn user_message_lines(message: &str, max_lines: usize) -> Vec<Line<'static>> {
    let body_max = max_lines.saturating_sub(1);
    let header = Line::from(Span::styled(
        "▌ You",
        Style::default().fg(ROLE_USER).add_modifier(Modifier::BOLD),
    ));
    let message_lines = message.lines().collect::<Vec<_>>();
    if message_lines.is_empty() {
        return vec![header];
    }
    let mut lines = vec![header];
    let hidden_count = truncated_line_count(message_lines.len(), body_max);
    let (head, tail) = head_tail_line_window(message_lines.as_slice(), body_max);
    if let Some(first) = head.first() {
        lines.push(Line::from(format!("  {first}")));
    }
    if hidden_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("    ... {} more line(s)", hidden_count),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
    lines.extend(tail.iter().map(|line| Line::from(format!("  {line}"))));
    lines
}

fn agent_message_lines(message: &str, max_lines: usize) -> Vec<Line<'static>> {
    let body_max = max_lines.saturating_sub(1);
    let mut lines = vec![Line::from(Span::styled(
        "▌ Agent",
        Style::default().fg(ROLE_AGENT).add_modifier(Modifier::BOLD),
    ))];
    let message_lines: Vec<&str> = message.lines().collect();
    if message_lines.is_empty() {
        return lines;
    }
    let hidden_count = truncated_line_count(message_lines.len(), body_max);
    let (head, tail) = head_tail_line_window(&message_lines, body_max);
    for line in head {
        lines.push(Line::from(format!("  {line}")));
    }
    if hidden_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("    ... {} more line(s)", hidden_count),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
    for line in tail {
        lines.push(Line::from(format!("  {line}")));
    }
    lines
}

fn system_message_lines(message: &str, max_lines: usize) -> Vec<Line<'static>> {
    let body_max = max_lines.saturating_sub(1);
    let mut lines = vec![Line::from(Span::styled(
        "▌ System",
        Style::default()
            .fg(ROLE_SYSTEM)
            .add_modifier(Modifier::BOLD),
    ))];
    let message_lines: Vec<&str> = message.lines().collect();
    if message_lines.is_empty() {
        return lines;
    }
    let hidden_count = truncated_line_count(message_lines.len(), body_max);
    let (head, tail) = head_tail_line_window(&message_lines, body_max);
    for line in head {
        lines.push(Line::from(format!("  {line}")));
    }
    if hidden_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("    ... {} more line(s)", hidden_count),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
    for line in tail {
        lines.push(Line::from(format!("  {line}")));
    }
    lines
}
pub(crate) fn formatted_message_lines(
    role: &str,
    message: &str,
    max_lines: usize,
    cwd: Option<&Path>,
) -> Vec<Line<'static>> {
    use crate::tui::message_role::MessageRole;
    let role_kind = MessageRole::try_from_str(role);
    if role_kind == Some(MessageRole::Agent) {
        let mut lines = vec![Line::from(Span::styled(
            "▌ Agent",
            Style::default().fg(ROLE_AGENT).add_modifier(Modifier::BOLD),
        ))];
        let body = bulleted_markdown_message_lines(message, max_lines, cwd);
        lines.extend(body);
        return lines;
    }
    if role_kind == Some(MessageRole::System) {
        let mut lines = vec![Line::from(Span::styled(
            "▌ System",
            Style::default()
                .fg(ROLE_SYSTEM)
                .add_modifier(Modifier::BOLD),
        ))];
        let body_max = max_lines.saturating_sub(1);
        let mut rendered = Vec::new();
        super::markdown::append_markdown(message, None, cwd, &mut rendered);
        let rendered_len = rendered.len();
        if !rendered.is_empty() {
            let hidden_count = truncated_line_count(rendered_len, body_max);
            let (head, tail) = head_tail_line_window(rendered.as_slice(), body_max);
            lines.extend(prefix_lines(
                head.iter().cloned().collect(),
                Span::raw("  "),
                Span::raw("  "),
            ));
            if hidden_count > 0 {
                lines.push(Line::from(Span::styled(
                    format!("    ... {} more line(s)", hidden_count),
                    Style::default().fg(TEXT_SECONDARY),
                )));
            }
            lines.extend(prefix_lines(
                tail.iter().cloned().collect(),
                Span::raw("  "),
                Span::raw("  "),
            ));
        }
        return lines;
    }
    prefixed_message_lines(role, message, max_lines)
}

fn bulleted_markdown_message_lines(
    message: &str,
    max_lines: usize,
    cwd: Option<&Path>,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    super::markdown::append_markdown(message, None, cwd, &mut rendered);
    let rendered_len = rendered.len();

    if rendered.is_empty() {
        return vec![Line::from("•")];
    }

    let hidden_count = truncated_line_count(rendered_len, max_lines);
    let (head, tail) = head_tail_line_window(rendered.as_slice(), max_lines);

    let mut lines = prefix_lines(
        head.iter().cloned().collect(),
        Span::styled("• ", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  "),
    );
    if hidden_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ... {} more line(s)", hidden_count),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.extend(prefix_lines(
        tail.iter().cloned().collect(),
        Span::raw("  "),
        Span::raw("  "),
    ));
    lines
}

fn markdown_message_lines(
    role: &str,
    message: &str,
    max_lines: usize,
    cwd: Option<&Path>,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    super::markdown::append_markdown(message, None, cwd, &mut rendered);
    let rendered_len = rendered.len();

    if rendered.is_empty() {
        return vec![Line::from(role.to_string())];
    }

    let mut lines = vec![Line::from(role.to_string())];
    let hidden_count = truncated_line_count(rendered_len, max_lines);
    let (head, tail) = head_tail_line_window(rendered.as_slice(), max_lines);
    let prefixed = prefix_lines(
        head.iter().cloned().collect(),
        Span::raw("  "),
        Span::raw("  "),
    );
    lines.extend(prefixed);
    if hidden_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ... {} more line(s)", hidden_count),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.extend(prefix_lines(
        tail.iter().cloned().collect(),
        Span::raw("  "),
        Span::raw("  "),
    ));
    lines
}

pub(crate) fn rendered_markdown_lines(
    role: &str,
    rendered: &[Line<'static>],
    max_lines: usize,
) -> Vec<Line<'static>> {
    if rendered.is_empty() {
        return vec![Line::from(role.to_string())];
    }

    let rendered_len = rendered.len();
    let capped = if max_lines == usize::MAX {
        rendered_len
    } else {
        max_lines.min(rendered_len)
    };

    let mut lines = vec![Line::from(role.to_string())];
    let prefixed = prefix_lines(
        rendered.iter().take(capped).cloned().collect(),
        Span::raw("  "),
        Span::raw("  "),
    );
    lines.extend(prefixed);
    if capped < rendered_len {
        lines.push(Line::from(Span::styled(
            format!("  ... {} more line(s)", rendered_len - capped),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

fn is_exploration_tool(name: &str) -> bool {
    matches!(name, "list_files" | "read_file" | "glob" | "grep")
}

fn exploration_action_label(message: &str) -> Option<String> {
    let mut parts = message.split_whitespace();
    let name = parts.next()?;
    let rest = parts.collect::<Vec<_>>().join(" ");
    match name {
        "read_file" => Some(format!(
            "Read {}",
            if rest.is_empty() {
                "file"
            } else {
                rest.as_str()
            }
        )),
        _ => None,
    }
}

fn tool_action_label(message: &str) -> Option<String> {
    let mut parts = message.split_whitespace();
    let name = parts.next()?;
    if is_exploration_tool(name) {
        return None;
    }

    let rest = parts.collect::<Vec<_>>().join(" ");
    match name {
        "bash" => Some(format!(
            "Run {}",
            if rest.is_empty() {
                "command"
            } else {
                rest.as_str()
            }
        )),
        "apply_patch" => Some(format!(
            "Apply patch {}",
            if rest.is_empty() {
                "changes"
            } else {
                rest.as_str()
            }
        )),
        "write_file" => Some(format!(
            "Write {}",
            if rest.is_empty() {
                "file"
            } else {
                rest.as_str()
            }
        )),
        "replace" => Some(format!(
            "Edit {}",
            if rest.is_empty() {
                "file"
            } else {
                rest.as_str()
            }
        )),
        "replace_lines" => Some(format!(
            "Edit lines {}",
            if rest.is_empty() {
                "in file"
            } else {
                rest.as_str()
            }
        )),
        "spawn_agent" => {
            let kind = SubAgentKind::from_tool_name(name).unwrap_or_else(|| {
                eprintln!("Warning: unknown sub-agent tool name in render: {name}");
                SubAgentKind::General
            });
            let (icon, _) = kind.action_icon();
            let delegate = compact_delegate_rest(&rest).unwrap_or_else(|| "sub-agent".to_string());
            Some(format!("{}{} {}", icon, kind.action_label(), delegate))
        }
        "explore_agent" | "plan_agent" | "team_create" => {
            let kind = SubAgentKind::from_tool_name(name).unwrap_or_else(|| {
                eprintln!("Warning: unknown sub-agent tool name in render: {name}");
                SubAgentKind::General
            });
            let (icon, _) = kind.action_icon();
            let abbreviation = compact_instruction(&rest);
            Some(format!("{}{} {}", icon, kind.action_label(), abbreviation))
        }
        "web_fetch" => Some(format!(
            "Fetch {}",
            if rest.is_empty() {
                "resource"
            } else {
                rest.as_str()
            }
        )),
        "web_search" => Some(format!(
            "Search {}",
            if rest.is_empty() {
                "web".to_string()
            } else {
                compact_instruction(&rest)
            }
        )),
        other => Some(format!(
            "Run {}",
            if rest.is_empty() { other } else { message }
        )),
    }
}
