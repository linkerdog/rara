use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use super::state::SearchStatus;
use crate::tui::composer_atoms::encode_mention;
use crate::tui::custom_terminal::Frame;
use crate::tui::display_sanitize::sanitize_display_line;
use crate::tui::state::TuiApp;
use crate::tui::text_wrap::truncate_to_width;
use crate::tui::theme::{ThemeToken, theme_color, token_fg};

/// Anchors above the composer where possible; tiny viewports may cover it.
pub(in crate::tui) fn render_file_mentions(
    f: &mut Frame,
    app: &TuiApp,
    main: Rect,
    composer_top: u16,
) -> Option<Rect> {
    if !app.file_mention_open() || main.is_empty() {
        return None;
    }
    let state = &app.file_mentions;
    let height = (state.matches.len().clamp(1, 6) as u16 + 2).min(main.height);
    let area = Rect::new(
        main.x,
        composer_top.saturating_sub(height).max(main.y),
        main.width,
        height,
    );
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(area);
    let style = token_fg(ThemeToken::TextPrimary).bg(theme_color(ThemeToken::PopupBg));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new("").style(style), area);
    let title = match state.status {
        SearchStatus::Searching => "Files | Searching…",
        SearchStatus::Failed => "Files | Search failed",
        SearchStatus::Ready {
            truncated: true, ..
        } => "Files | Index limit reached",
        SearchStatus::Ready {
            skipped_non_utf8: true,
            ..
        } => "Files | Non-UTF-8 paths omitted",
        SearchStatus::Ready { .. } => "Files",
    };
    f.render_widget(
        Paragraph::new(title).style(token_fg(ThemeToken::TextAccent)),
        chunks[0],
    );
    if state.matches.is_empty() {
        let label = match state.status {
            SearchStatus::Searching => "Searching workspace…",
            SearchStatus::Failed => "See search error in conversation",
            SearchStatus::Ready { .. } => "No matching files",
        };
        f.render_widget(
            Paragraph::new(label).style(token_fg(ThemeToken::TextMuted)),
            chunks[1],
        );
    } else {
        let items = state
            .matches
            .iter()
            .map(|entry| {
                let path = encode_mention(&entry.path.to_string_lossy());
                let safe = sanitize_display_line(&path);
                ListItem::new(
                    truncate_to_width(&safe, chunks[1].width.saturating_sub(2) as usize).to_owned(),
                )
            })
            .collect::<Vec<_>>();
        let mut selection = ListState::default().with_selected(Some(state.selected));
        f.render_stateful_widget(
            List::new(items).highlight_symbol("> ").highlight_style(
                Style::default()
                    .fg(theme_color(ThemeToken::OverlayHighlightFg))
                    .bg(theme_color(ThemeToken::OverlayHighlightBg)),
            ),
            chunks[1],
            &mut selection,
        );
    }
    f.render_widget(
        Paragraph::new("↑/↓ select  Tab/Enter insert  Esc back")
            .style(token_fg(ThemeToken::TextMuted)),
        chunks[2],
    );
    Some(area)
}
