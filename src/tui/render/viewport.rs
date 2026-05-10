use std::cmp;

use ratatui::{
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Wrap},
};

use crate::tui::custom_terminal::Frame;

/// Pre-computed metadata for fast scroll-window lookups.
///
/// `row_boundaries[i]` is the cumulative visual row count *after*
/// line i (exclusive), i.e. how many visual rows the first (i+1)
/// lines occupy.
#[derive(Clone, Debug)]
pub(crate) struct LineLayout {
    pub(crate) line_count: usize,
    pub(crate) total_rows: usize,
    /// `row_boundaries[i]` = total visual rows occupied by lines 0..=i
    row_boundaries: Vec<usize>,
}

impl LineLayout {
    pub(crate) fn compute(lines: &[Line<'static>], wrap_width: usize) -> Self {
        let line_count = lines.len();
        let mut row_boundaries = Vec::with_capacity(line_count);
        let mut acc = 0usize;
        for line in lines {
            let rows = visual_rows_for_line(line, wrap_width);
            acc = acc.saturating_add(rows);
            row_boundaries.push(acc);
        }
        let total_rows = acc;
        Self {
            line_count,
            total_rows,
            row_boundaries,
        }
    }

    fn rows_before_line(&self, idx: usize) -> usize {
        if idx == 0 {
            0
        } else {
            self.row_boundaries[idx - 1]
        }
    }

    fn first_line_past(&self, target_row: usize) -> usize {
        self.row_boundaries
            .binary_search_by(|&boundary| {
                if boundary > target_row {
                    cmp::Ordering::Greater
                } else {
                    cmp::Ordering::Less
                }
            })
            .unwrap_or_else(|i| i)
    }
}

pub(crate) struct TranscriptViewport {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) scroll_offset: u16,
    /// Pre-computed layout; recomputed when lines change.
    layout: LineLayout,
}

impl TranscriptViewport {
    pub(crate) fn new(lines: Vec<Line<'static>>, scroll_offset: u16, width: u16) -> Self {
        let layout = LineLayout::compute(&lines, usize::from(width));
        Self {
            lines,
            scroll_offset,
            layout,
        }
    }

    pub(crate) fn update_lines(&mut self, lines: Vec<Line<'static>>, width: u16) {
        self.layout = LineLayout::compute(&lines, usize::from(width));
        self.lines = lines;
    }

    /// O(log n) scroll window using pre-computed row boundaries.
    pub(crate) fn visible_window(&self, _width: u16, height: u16) -> (Vec<Line<'static>>, u16) {
        if self.lines.is_empty() || height == 0 {
            return (Vec::new(), 0);
        }

        // Reserve one bottom row as breathing room so content isn't
        // visually flush against the input bar at any scroll position.
        let visible_rows = usize::from(height.saturating_sub(1).max(1));
        let target_start = usize::from(self.scroll_offset);
        let target_end = target_start.saturating_add(visible_rows);

        if target_start >= self.layout.total_rows {
            return (Vec::new(), 0);
        }

        let first_idx = self.layout.first_line_past(target_start);
        let first_inner_scroll =
            target_start.saturating_sub(self.layout.rows_before_line(first_idx)) as u16;
        let last_exclusive_idx = (self.layout.first_line_past(target_end.saturating_sub(1)) + 1)
            .min(self.layout.line_count);

        (
            self.lines[first_idx..last_exclusive_idx.max(first_idx + 1).min(self.lines.len())]
                .to_vec(),
            first_inner_scroll,
        )
    }

    pub(crate) fn render(&self, f: &mut Frame, area: Rect) {
        let (visible_lines, inner_scroll) = self.visible_window(area.width, area.height);
        f.render_widget(
            Paragraph::new(visible_lines)
                .wrap(Wrap { trim: false })
                .scroll((inner_scroll, 0)),
            area,
        );
    }
}

fn visual_rows_for_line(line: &Line<'static>, wrap_width: usize) -> usize {
    crate::tui::layout_utils::line_visual_rows(line, wrap_width)
}
