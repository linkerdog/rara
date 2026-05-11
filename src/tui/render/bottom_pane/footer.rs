// Footer — renders the bottom line of the bottom pane from a FooterView.
use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::Line,
    widgets::Paragraph,
};

use super::super::super::custom_terminal::Frame;
use super::bottom_pane_style;
use crate::tui::theme::TEXT_SECONDARY;

pub(super) fn render_footer(f: &mut Frame, view: &super::view::FooterView, area: Rect) {
    if view.hide {
        let para = Paragraph::new("").style(bottom_pane_style());
        f.render_widget(para, area);
        return;
    }
    let line = Line::styled(view.text.as_str(), Style::default().fg(TEXT_SECONDARY));
    let para = Paragraph::new(line)
        .alignment(Alignment::Right)
        .style(bottom_pane_style());
    f.render_widget(para, area);
}
