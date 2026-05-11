// Footer — renders the bottom line of the bottom pane from a FooterView.
use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::super::custom_terminal::Frame;
use super::bottom_pane_style;
use super::view::FooterView;
use crate::tui::theme::TEXT_SECONDARY;

pub(super) fn render_footer(f: &mut Frame, view: &FooterView, area: Rect) {
    if view.hide {
        f.render_widget(Paragraph::new("").style(bottom_pane_style()), area);
        return;
    }
    let summary = view.parts.join("  ");
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
