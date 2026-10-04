use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::tui::composer_text::{WrapConfig, wrapped_text};
use crate::tui::custom_terminal::Frame;
use crate::tui::display_sanitize::{sanitize_display_line, sanitize_display_text};
use crate::tui::state::TuiApp;
use crate::tui::text_wrap::truncate_to_width;
use crate::tui::theme::{ThemeToken, theme_color, token_fg};

pub(in crate::tui) fn render_history_search(
    f: &mut Frame,
    app: &TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    if area.is_empty() {
        return None;
    }
    let matches = app.history_matches();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Percentage(40),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    let loading = if app.prompt_history_loading() {
        " | Loading…"
    } else {
        ""
    };
    f.render_widget(
        Paragraph::new(format!(
            "Search prompts | {} matches{loading}",
            matches.len()
        ))
        .style(Style::default().add_modifier(Modifier::BOLD)),
        chunks[0],
    );

    let query = wrapped_text(
        &app.prompt_history.query,
        WrapConfig {
            width: chunks[1].width.max(1),
            initial_indent: "> ",
            subsequent_indent: "  ",
        },
    );
    let offset = app
        .prompt_history
        .query_cursor
        .unwrap_or_else(|| app.prompt_history.query.chars().count());
    let position = query.cursor_position(offset);
    let first_row = position
        .row
        .saturating_sub(chunks[1].height.saturating_sub(1) as usize);
    let query_lines: Vec<_> = query
        .rows()
        .iter()
        .skip(first_row)
        .take(chunks[1].height as usize)
        .map(|row| Line::from(sanitize_display_line(row)))
        .collect();
    f.render_widget(Paragraph::new(query_lines), chunks[1]);

    if matches.is_empty() {
        let label = if app.prompt_history_loading() {
            "Loading prompt history…"
        } else if app.input_history.is_empty() {
            "No history yet"
        } else {
            "No matching prompts"
        };
        f.render_widget(
            Paragraph::new(label).style(token_fg(ThemeToken::TextMuted)),
            chunks[2],
        );
    } else {
        let items = matches
            .iter()
            .map(|text| {
                let first = sanitize_display_line(text.lines().next().unwrap_or_default());
                ListItem::new(
                    truncate_to_width(&first, chunks[2].width.saturating_sub(2) as usize)
                        .to_owned(),
                )
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default().with_selected(Some(app.prompt_history.selected));
        f.render_stateful_widget(
            List::new(items).highlight_symbol("> ").highlight_style(
                Style::default()
                    .fg(theme_color(ThemeToken::OverlayHighlightFg))
                    .bg(theme_color(ThemeToken::OverlayHighlightBg)),
            ),
            chunks[2],
            &mut state,
        );
    }

    if let Some(text) = matches.get(app.prompt_history.selected) {
        let preview = sanitize_display_text(text);
        let wrapped = wrapped_text(
            &preview,
            WrapConfig {
                width: chunks[3].width.max(1),
                initial_indent: "",
                subsequent_indent: "",
            },
        );
        let height = chunks[3].height as usize;
        let omitted = wrapped.rows().len() > height;
        let visible = height.saturating_sub(usize::from(omitted));
        let mut lines: Vec<_> = wrapped
            .rows()
            .iter()
            .take(visible)
            .cloned()
            .map(Line::from)
            .collect();
        if omitted && height > 0 {
            lines.push(Line::from(format!(
                "… {} more lines",
                wrapped.rows().len() - visible
            )));
        }
        f.render_widget(
            Paragraph::new(lines).style(token_fg(ThemeToken::TextSecondary)),
            chunks[3],
        );
    }
    f.render_widget(
        Paragraph::new("Ctrl+R/Up older  Down newer  Enter use  Esc back")
            .style(token_fg(ThemeToken::TextMuted)),
        chunks[4],
    );
    (!chunks[1].is_empty()).then_some((
        chunks[1].x.saturating_add(position.column as u16),
        chunks[1]
            .y
            .saturating_add((position.row - first_row) as u16),
    ))
}
