use ratatui::text::Line;

/// Number of terminal rows a line occupies when wrapped at `wrap_width`.
pub(crate) fn line_visual_rows(line: &Line<'_>, wrap_width: usize) -> usize {
    line.width().max(1).div_ceil(wrap_width)
}

/// Sum of visual rows across all lines.
pub(crate) fn total_visual_rows(lines: &[Line<'_>], width: u16) -> usize {
    let wrap_width = width.max(1) as usize;
    lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(wrap_width))
        .sum()
}
