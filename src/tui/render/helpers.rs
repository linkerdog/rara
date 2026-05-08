use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::tui::state::TuiApp;
use crate::tui::theme::*;

pub(crate) fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

pub(crate) fn display_directory_for_startup(app: &TuiApp) -> String {
    let cwd = if app.snapshot.cwd.is_empty() {
        std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| ".".to_string())
    } else {
        app.snapshot.cwd.clone()
    };
    if let Ok(home) = std::env::var("HOME") {
        if let Some(stripped) = cwd.strip_prefix(&home) {
            return format!("~{stripped}");
        }
    }
    cwd
}

pub(crate) fn truncate_for_startup_card(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let kept = value.chars().take(width - 1).collect::<String>();
    format!("{kept}…")
}

pub(crate) fn truncate_path_middle(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    if width <= 5 {
        return truncate_for_startup_card(value, width);
    }

    let keep_left = (width - 1) / 2;
    let keep_right = width - 1 - keep_left;
    let chars = value.chars().collect::<Vec<_>>();
    let left = chars.iter().take(keep_left).collect::<String>();
    let right = chars
        .iter()
        .rev()
        .take(keep_right)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{left}…{right}")
}

pub(crate) fn startup_card_inner_width(width: u16) -> Option<usize> {
    if width < 8 {
        return None;
    }
    Some(std::cmp::min(width.saturating_sub(4) as usize, 56))
}

/// Lightweight container — adds a thin top rule above content instead of a heavy border box.
pub(crate) fn with_border(lines: Vec<Line<'static>>, inner_width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(lines.len() + 2);
    let top_rule = format!("──{}", "─".repeat(inner_width.saturating_sub(2)));
    out.push(Line::from(Span::styled(
        top_rule,
        Style::default().fg(TEXT_MUTED),
    )));

    for line in lines {
        let used_width = line
            .iter()
            .map(|span| display_width(span.content.as_ref()))
            .sum::<usize>();
        let mut spans = Vec::with_capacity(line.spans.len() + 3);
        spans.push(Span::from("  "));
        spans.extend(line.into_iter());
        if used_width < inner_width {
            spans.push(Span::from(" ".repeat(inner_width - used_width)));
        }
        out.push(Line::from(spans));
    }
    out
}

pub(crate) fn badge<'a>(label: &'a str, value: &'a str, color: Color) -> Span<'a> {
    let fg = match color {
        Color::Black
        | Color::DarkGray
        | Color::Gray
        | Color::Blue
        | Color::Red
        | Color::Magenta => Color::White,
        _ => Color::Black,
    };
    Span::styled(
        format!(" {}={} ", label, value),
        Style::default()
            .fg(fg)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

/// Lightweight section label — colored foreground, no heavy background badge.
pub(crate) fn section_label(title: &str, color: Color) -> Span<'static> {
    Span::styled(format!("# {title}"), Style::default().fg(color))
}

pub(crate) fn wrapped_history_line_count(lines: &[Line<'static>], width: u16) -> u16 {
    crate::tui::layout_utils::wrapped_line_count(lines, width)
}
