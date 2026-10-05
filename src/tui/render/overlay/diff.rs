use ratatui::{layout::Rect, text::Line, widgets::Paragraph};

use crate::tui::custom_terminal::Frame;
use crate::tui::state::TuiApp;
use crate::tui::theme::{ThemeToken, token_fg};

pub(super) fn render(frame: &mut Frame, app: &TuiApp, area: Rect) {
    let block = super::popup_block().title(" Working tree diff ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let body = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let logical = app
        .diff_view
        .text
        .lines()
        .map(|line| {
            let token = if line.starts_with("@@") || line.starts_with("diff --git") {
                ThemeToken::DiffHunkFg
            } else if line.starts_with('+') {
                ThemeToken::DiffAddFg
            } else if line.starts_with('-') {
                ThemeToken::DiffDelFg
            } else {
                ThemeToken::TextPrimary
            };
            Line::styled(line.to_owned(), token_fg(token))
        })
        .collect::<Vec<_>>();
    let rows = crate::tui::transcript_text::wrap_lines(&logical, body.width);
    app.diff_view.rows.set(rows.len());
    app.diff_view.height.set(usize::from(body.height));
    let max = rows.len().saturating_sub(usize::from(body.height));
    let offset = app.diff_view.offset.get().min(max);
    app.diff_view.offset.set(offset);
    frame.render_widget(
        Paragraph::new(
            rows.into_iter()
                .skip(offset)
                .take(usize::from(body.height))
                .collect::<Vec<_>>(),
        ),
        body,
    );
    if inner.height > 0 {
        let footer = Rect {
            y: body.bottom(),
            height: 1,
            ..inner
        };
        let hint = if inner.width >= 52 {
            "Esc close | arrows / PgUp PgDn / Home End scroll"
        } else if inner.width >= 28 {
            "Esc close  arrows / PgUp PgDn"
        } else {
            "Esc close"
        };
        frame.render_widget(
            Paragraph::new(hint).style(token_fg(ThemeToken::TextMuted)),
            footer,
        );
    }
}
