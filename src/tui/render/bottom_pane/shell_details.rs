use ratatui::{layout::Rect, style::Style, text::Line, widgets::Paragraph};

use super::{bottom_pane_style, view};
use crate::tui::custom_terminal::Frame;
use crate::tui::display_sanitize::sanitize_display_text;
use crate::tui::state::ApprovalDetailScroll;
use crate::tui::text_wrap::truncate_to_width;
use crate::tui::theme::TEXT_SECONDARY;

pub(super) fn render(
    f: &mut Frame,
    panel: &view::InteractionPanelView,
    approval: &view::ShellApprovalView,
    details: Vec<Line<'static>>,
    scroll: &mut ApprovalDetailScroll,
    area: Rect,
) {
    // Keep at least one content row before allocating optional chrome.
    let title_rows = u16::from(area.height >= 4);
    let directory_rows = u16::from(area.height >= 3);
    let hint_rows = u16::from(area.height >= 2);
    let content_rows = area
        .height
        .saturating_sub(title_rows + directory_rows + hint_rows);
    let range = scroll.visible_range(
        &approval.tool_use_id,
        details.len(),
        usize::from(content_rows),
    );
    let mut lines = Vec::with_capacity(usize::from(area.height));
    if title_rows > 0 {
        lines.push(Line::styled(
            format!("  # {}", panel.title),
            Style::default().fg(panel.color),
        ));
    }
    let total_rows = details.len();
    lines.extend(details.into_iter().skip(range.start).take(range.len()));
    lines.resize_with(usize::from(title_rows + content_rows), Line::default);
    if directory_rows > 0 {
        let directory = format!(
            "  cwd: {}",
            sanitize_display_text(&approval.cwd).replace('\n', " ")
        );
        let prefix = truncate_to_width(&directory, usize::from(area.width));
        let directory = if prefix.len() < directory.len() {
            format!(
                "{}…",
                truncate_to_width(&directory, usize::from(area.width.saturating_sub(1)))
            )
        } else {
            directory
        };
        lines.push(Line::styled(directory, Style::default().fg(TEXT_SECONDARY)));
    }
    if hint_rows > 0 {
        lines.push(Line::styled(
            format!(
                "  {}-{}/{} PgUp/PgDn Home/End",
                range.start.saturating_add(1),
                range.end,
                total_rows,
            ),
            Style::default().fg(TEXT_SECONDARY),
        ));
    }
    f.render_widget(Paragraph::new(lines).style(bottom_pane_style()), area);
}
