use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::Range,
};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
};
use unicode_segmentation::UnicodeSegmentation;

use super::text_wrap::display_width;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScreenPosition {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

impl ScreenPosition {
    pub(crate) fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptSelection {
    anchor: Option<SelectionPoint>,
    focus: Option<SelectionPoint>,
    last_mouse: Option<ScreenPosition>,
    dragging: bool,
    snapshot: TranscriptSelectionSnapshot,
    snapshot_key: Option<TranscriptSelectionSnapshotKey>,
}

impl TranscriptSelection {
    pub(crate) fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub(crate) fn update_snapshot(
        &mut self,
        lines: &[Line<'static>],
        area: Rect,
        scroll_offset: u16,
    ) {
        let key = TranscriptSelectionSnapshotKey::from_lines(lines, area, scroll_offset);
        if self.snapshot_key == Some(key) {
            return;
        }
        self.snapshot =
            TranscriptSelectionSnapshot::from_lines(lines, area, usize::from(scroll_offset));
        self.snapshot_key = Some(key);
        self.clamp_points_to_snapshot();
    }

    pub(crate) fn clear_snapshot(&mut self) {
        self.snapshot = TranscriptSelectionSnapshot::default();
        self.snapshot_key = None;
        self.clear();
    }

    pub(crate) fn start(&mut self, position: ScreenPosition) -> bool {
        let Some(point) = self
            .snapshot
            .point_for_position(position, PointClamp::InsideOnly)
        else {
            return false;
        };
        self.anchor = Some(point);
        self.focus = Some(point);
        self.last_mouse = Some(position);
        self.dragging = true;
        true
    }

    pub(crate) fn drag(&mut self, position: ScreenPosition) -> bool {
        if !self.dragging {
            return false;
        }
        self.last_mouse = Some(position);
        if let Some(point) = self
            .snapshot
            .point_for_position(position, PointClamp::ClampToArea)
        {
            self.focus = Some(point);
        }
        true
    }

    pub(crate) fn finish(&mut self, position: ScreenPosition) -> Option<String> {
        if !self.dragging {
            return None;
        }
        let _ = self.drag(position);
        let selected = self.selected_text();
        self.clear();
        selected
    }

    pub(crate) fn clear(&mut self) {
        self.anchor = None;
        self.focus = None;
        self.last_mouse = None;
        self.dragging = false;
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        let (start, end) = self.normalized_range()?;
        self.snapshot.text_for_range(start, end)
    }

    pub(crate) fn autoscroll_delta(&mut self) -> Option<i32> {
        if !self.dragging || self.snapshot.rows.is_empty() {
            return None;
        }
        let last = self.last_mouse?;
        let area = self.snapshot.area;
        if last.y < area.y && self.snapshot.visible_start > 0 {
            let next_row = self.snapshot.visible_start.saturating_sub(1);
            self.focus = Some(self.snapshot.point_for_row_and_x(next_row, last.x));
            return Some(-1);
        }
        if last.y >= area.bottom() && self.snapshot.visible_end < self.snapshot.rows.len() {
            let next_row = self.snapshot.visible_end.min(self.snapshot.rows.len() - 1);
            self.focus = Some(self.snapshot.point_for_row_and_x(next_row, last.x));
            return Some(1);
        }
        None
    }

    pub(crate) fn highlight_visible_range(&self, buffer: &mut Buffer) {
        let Some((start, end)) = self.normalized_range() else {
            return;
        };
        let style = Style::default().add_modifier(Modifier::REVERSED);
        for (row_index, row) in self.snapshot.visible_rows().iter().enumerate() {
            let row_start = if row.global_row == start.row {
                start.col
            } else {
                0
            };
            let row_end = if row.global_row == end.row {
                end.col
            } else {
                display_width(row.text)
            };
            if row.global_row < start.row || row.global_row > end.row || row_start >= row_end {
                continue;
            }
            let columns = selected_graphemes(row.text, row_start..row_end).columns;
            let y = self.snapshot.area.y.saturating_add(row_index as u16);
            if y >= self.snapshot.area.bottom() {
                break;
            }
            let x_start = self
                .snapshot
                .area
                .x
                .saturating_add(columns.start.min(u16::MAX as usize) as u16);
            let x_end = self
                .snapshot
                .area
                .x
                .saturating_add(columns.end.min(usize::from(self.snapshot.area.width)) as u16)
                .min(self.snapshot.area.right());
            for x in x_start..x_end {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_style(cell.style().patch(style));
                }
            }
        }
    }

    fn normalized_range(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let anchor = self.anchor?;
        let focus = self.focus?;
        if anchor == focus {
            return None;
        }
        if anchor <= focus {
            Some((anchor, focus))
        } else {
            Some((focus, anchor))
        }
    }

    fn clamp_points_to_snapshot(&mut self) {
        if self.snapshot.rows.is_empty() {
            self.clear();
            return;
        }
        self.anchor = self.anchor.map(|point| self.snapshot.clamp_point(point));
        self.focus = self.focus.map(|point| self.snapshot.clamp_point(point));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct SelectionPoint {
    row: usize,
    col: usize,
}

#[derive(Clone, Debug, Default)]
struct TranscriptSelectionSnapshot {
    area: Rect,
    rows: Vec<VisualRow>,
    visible_start: usize,
    visible_end: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TranscriptSelectionSnapshotKey {
    area: Rect,
    scroll_offset: u16,
    line_count: usize,
    span_count: usize,
    text_len: usize,
    content_hash: u64,
}

impl TranscriptSelectionSnapshotKey {
    fn from_lines(lines: &[Line<'static>], area: Rect, scroll_offset: u16) -> Self {
        let mut span_count = 0usize;
        let mut text_len = 0usize;
        let mut hasher = DefaultHasher::new();
        for line in lines {
            span_count = span_count.saturating_add(line.spans.len());
            for span in &line.spans {
                text_len = text_len.saturating_add(span.content.len());
            }
            hash_line(line, &mut hasher);
        }

        Self {
            area,
            scroll_offset,
            line_count: lines.len(),
            span_count,
            text_len,
            content_hash: hasher.finish(),
        }
    }
}

impl TranscriptSelectionSnapshot {
    fn from_lines(lines: &[Line<'static>], area: Rect, scroll_offset: usize) -> Self {
        let rows = lines
            .iter()
            .map(|line| VisualRow {
                text: line.to_string(),
            })
            .collect::<Vec<_>>();

        let visible_start = scroll_offset.min(rows.len());
        let visible_end = rows
            .len()
            .min(visible_start.saturating_add(usize::from(area.height)));
        Self {
            area,
            rows,
            visible_start,
            visible_end,
        }
    }

    fn visible_rows(&self) -> Vec<VisibleRow<'_>> {
        self.rows[self.visible_start.min(self.rows.len())..self.visible_end.min(self.rows.len())]
            .iter()
            .enumerate()
            .map(|(idx, row)| VisibleRow {
                global_row: self.visible_start + idx,
                text: row.text.as_str(),
            })
            .collect()
    }

    fn point_for_position(
        &self,
        position: ScreenPosition,
        clamp: PointClamp,
    ) -> Option<SelectionPoint> {
        if self.rows.is_empty() || self.area.width == 0 || self.area.height == 0 {
            return None;
        }
        let inside = position.x >= self.area.x
            && position.x < self.area.right()
            && position.y >= self.area.y
            && position.y < self.area.bottom();
        if !inside && matches!(clamp, PointClamp::InsideOnly) {
            return None;
        }
        let local_y = position
            .y
            .saturating_sub(self.area.y)
            .min(self.area.height.saturating_sub(1));
        let row = (self.visible_start + usize::from(local_y)).min(self.rows.len() - 1);
        Some(self.point_for_row_and_x(row, position.x))
    }

    fn point_for_row_and_x(&self, row: usize, x: u16) -> SelectionPoint {
        let row = row.min(self.rows.len().saturating_sub(1));
        let local_x = x.saturating_sub(self.area.x);
        let col = usize::from(local_x).min(display_width(self.rows[row].text.as_str()));
        SelectionPoint { row, col }
    }

    fn clamp_point(&self, point: SelectionPoint) -> SelectionPoint {
        let row = point.row.min(self.rows.len().saturating_sub(1));
        let col = point.col.min(display_width(self.rows[row].text.as_str()));
        SelectionPoint { row, col }
    }

    fn text_for_range(&self, start: SelectionPoint, end: SelectionPoint) -> Option<String> {
        if start == end || self.rows.is_empty() {
            return None;
        }
        let mut selected = Vec::new();
        for row_index in start.row..=end.row {
            let row = self.rows.get(row_index)?;
            let row_width = display_width(row.text.as_str());
            let start_col = if row_index == start.row { start.col } else { 0 };
            let end_col = if row_index == end.row {
                end.col
            } else {
                row_width
            };
            selected.push(slice_display_cols(
                row.text.as_str(),
                start_col.min(row_width),
                end_col.min(row_width),
            ));
        }
        while selected.first().is_some_and(String::is_empty) {
            selected.remove(0);
        }
        while selected.last().is_some_and(String::is_empty) {
            selected.pop();
        }
        let text = selected.join("\n");
        (!text.is_empty()).then_some(text)
    }
}

#[derive(Clone, Copy)]
enum PointClamp {
    InsideOnly,
    ClampToArea,
}

#[derive(Clone, Debug)]
struct VisualRow {
    text: String,
}

#[derive(Clone, Copy)]
struct VisibleRow<'a> {
    global_row: usize,
    text: &'a str,
}

fn slice_display_cols(text: &str, start: usize, end: usize) -> String {
    text[selected_graphemes(text, start..end).bytes].to_string()
}

struct SelectedGraphemes {
    columns: Range<usize>,
    bytes: Range<usize>,
}

fn selected_graphemes(text: &str, range: Range<usize>) -> SelectedGraphemes {
    let mut selected = SelectedGraphemes {
        columns: 0..0,
        bytes: 0..0,
    };
    if range.is_empty() {
        return selected;
    }
    let mut col = 0usize;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let width = display_width(grapheme);
        let next = col.saturating_add(width);
        if next > range.start && col < range.end {
            if selected.bytes.is_empty() {
                selected.bytes.start = offset;
                selected.columns.start = col;
            }
            selected.bytes.end = offset + grapheme.len();
            selected.columns.end = next;
        }
        col = next;
        if col >= range.end {
            break;
        }
    }
    selected
}

fn hash_line(line: &Line<'static>, hasher: &mut DefaultHasher) {
    line.spans.len().hash(hasher);
    for span in &line.spans {
        span.content.hash(hasher);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{layout::Rect, text::Line};

    use super::{ScreenPosition, TranscriptSelection};

    #[test]
    fn selection_extracts_wrapped_visible_text() {
        let mut selection = TranscriptSelection::default();
        selection.update_snapshot(
            &crate::tui::transcript_text::wrap_lines(&[Line::from("abcdef"), Line::from("gh")], 3),
            Rect::new(0, 0, 3, 3),
            0,
        );

        assert!(selection.start(ScreenPosition::new(1, 0)));
        assert!(selection.drag(ScreenPosition::new(2, 1)));

        assert_eq!(selection.selected_text().as_deref(), Some("bc\nde"));
    }

    #[test]
    fn autoscroll_extends_selection_beyond_visible_area() {
        let mut selection = TranscriptSelection::default();
        selection.update_snapshot(
            &[Line::from("one"), Line::from("two"), Line::from("three")],
            Rect::new(0, 1, 10, 2),
            1,
        );

        assert!(selection.start(ScreenPosition::new(0, 2)));
        assert!(selection.drag(ScreenPosition::new(0, 1)));

        assert_eq!(selection.autoscroll_delta(), None);
        assert!(selection.drag(ScreenPosition::new(0, 0)));
        assert_eq!(selection.autoscroll_delta(), Some(-1));
        assert_eq!(selection.selected_text().as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn snapshot_cache_refreshes_when_transcript_edges_change() {
        let mut selection = TranscriptSelection::default();
        let area = Rect::new(0, 0, 10, 2);
        selection.update_snapshot(&[Line::from("one")], area, 0);
        assert!(selection.start(ScreenPosition::new(0, 0)));
        assert!(selection.drag(ScreenPosition::new(3, 0)));
        assert_eq!(selection.selected_text().as_deref(), Some("one"));

        selection.update_snapshot(&[Line::from("two")], area, 0);
        assert_eq!(selection.selected_text().as_deref(), Some("two"));
    }

    #[test]
    fn snapshot_cache_refreshes_same_size_middle_rows() {
        let mut selection = TranscriptSelection::default();
        let area = Rect::new(0, 0, 10, 3);
        selection.update_snapshot(
            &[Line::from("first"), Line::from("old"), Line::from("last")],
            area,
            0,
        );
        assert!(selection.start(ScreenPosition::new(0, 1)));
        assert!(selection.drag(ScreenPosition::new(3, 1)));
        assert_eq!(selection.selected_text().as_deref(), Some("old"));
        selection.update_snapshot(
            &[Line::from("first"), Line::from("new"), Line::from("last")],
            area,
            0,
        );
        assert_eq!(selection.selected_text().as_deref(), Some("new"));
    }

    #[test]
    fn snapshot_cache_tracks_row_boundaries_when_span_totals_are_unchanged() {
        use ratatui::text::Span;

        let mut selection = TranscriptSelection::default();
        let area = Rect::new(0, 0, 10, 2);
        selection.update_snapshot(
            &[
                Line::from(vec![Span::raw("a"), Span::raw("b")]),
                Line::from("c"),
            ],
            area,
            0,
        );
        assert!(selection.start(ScreenPosition::new(0, 0)));
        assert!(selection.drag(ScreenPosition::new(2, 0)));
        assert_eq!(selection.selected_text().as_deref(), Some("ab"));
        selection.update_snapshot(
            &[
                Line::from("a"),
                Line::from(vec![Span::raw("b"), Span::raw("c")]),
            ],
            area,
            0,
        );
        assert_eq!(selection.selected_text().as_deref(), Some("a"));
    }
}
