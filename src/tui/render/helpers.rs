use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

use crate::tui::display_sanitize::sanitize_display_line_segments;
use crate::tui::state::TuiApp;
pub(crate) use crate::tui::text_wrap::display_width;
use crate::tui::text_wrap::{suffix_to_width, truncate_to_width};
use crate::tui::theme::*;

pub(crate) fn display_directory_for_startup(app: &TuiApp) -> String {
    let cwd = if app.snapshot.cwd.is_empty() {
        std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| ".".to_string())
    } else {
        app.snapshot.cwd.clone()
    };
    if let Ok(home) = std::env::var("HOME")
        && let Some(stripped) = cwd.strip_prefix(&home)
    {
        return format!("~{stripped}");
    }
    cwd
}

pub(crate) fn truncate_for_startup_card(value: &str, width: usize) -> String {
    let value = sanitize_display_line_segments(&ratatui::text::Line::from(value)).to_string();
    if width == 0 {
        return String::new();
    }
    if display_width(&value) <= width {
        return value;
    }
    if width <= 1 {
        return "…".to_string();
    }
    let kept = truncate_to_width(&value, width - 1);
    format!("{kept}…")
}

pub(crate) fn truncate_path_middle(value: &str, width: usize) -> String {
    let value = sanitize_display_line_segments(&ratatui::text::Line::from(value)).to_string();
    if width == 0 {
        return String::new();
    }
    if display_width(&value) <= width {
        return value;
    }
    if width <= 1 {
        return "…".to_string();
    }
    if width <= 5 {
        return truncate_for_startup_card(&value, width);
    }

    let keep_left = (width - 1) / 2;
    let keep_right = width - 1 - keep_left;
    let left = truncate_to_width(&value, keep_left);
    let right = suffix_to_width(&value, keep_right);
    format!("{left}…{right}")
}

pub(crate) fn startup_card_inner_width(width: u16) -> Option<usize> {
    if width < 8 {
        return None;
    }
    Some(std::cmp::min(width.saturating_sub(4) as usize, 56))
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
