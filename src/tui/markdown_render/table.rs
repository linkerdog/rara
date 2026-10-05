//! Canonical styled table collection and width-aware rendering.

use pulldown_cmark::Alignment;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::tui::{
    display_sanitize::sanitize_display_line_segments,
    text_wrap::{display_width, grapheme_width},
    transcript_text::wrap_line,
};

#[derive(Debug)]
pub(super) struct TableRenderState {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Line<'static>>>,
    current_row: Option<Vec<Line<'static>>>,
    current_cell: Option<Line<'static>>,
    header_row_count: usize,
}

impl TableRenderState {
    pub(super) fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            rows: Vec::new(),
            current_row: None,
            current_cell: None,
            header_row_count: 0,
        }
    }

    pub(super) fn start_row(&mut self) {
        self.current_row = Some(Vec::new());
    }

    pub(super) fn end_row(&mut self) {
        if let Some(row) = self.current_row.take() {
            self.rows.push(row);
        }
    }

    pub(super) fn start_header(&mut self) {
        if self.current_row.is_none() {
            self.start_row();
        }
    }

    pub(super) fn end_header(&mut self) {
        if self.current_cell.is_some() {
            self.end_cell();
        }
        if self.current_row.is_some() {
            self.end_row();
        }
        self.header_row_count = self.rows.len();
    }

    pub(super) fn start_cell(&mut self) {
        self.current_cell = Some(Line::default());
    }

    pub(super) fn end_cell(&mut self) {
        let cell = self.current_cell.take().unwrap_or_default();
        if let Some(row) = self.current_row.as_mut() {
            // Measure exactly the visible content that the shared wrapper will use.
            row.push(sanitize_display_line_segments(&cell));
        }
    }

    pub(super) fn push_span(&mut self, span: Span<'static>) {
        if let Some(cell) = self.current_cell.as_mut() {
            cell.push_span(span);
        }
    }
}

pub(super) fn render_table_lines(
    table: &TableRenderState,
    width: Option<usize>,
) -> Vec<Line<'static>> {
    let column_count = table.rows.iter().map(Vec::len).max().unwrap_or(0);
    if column_count == 0 {
        return Vec::new();
    }

    let mut ideal = vec![1; column_count];
    let mut minimum = vec![1; column_count];
    for row in &table.rows {
        for (idx, cell) in row.iter().enumerate() {
            let text = cell.to_string();
            ideal[idx] = ideal[idx].max(display_width(&text));
            minimum[idx] =
                minimum[idx].max(text.graphemes(true).map(grapheme_width).max().unwrap_or(1));
        }
    }
    let Some(widths) = fit_column_widths(&ideal, &minimum, width) else {
        return render_records(table, width.unwrap_or(1).max(1));
    };

    let mut lines = Vec::new();
    for (row_idx, row) in table.rows.iter().enumerate() {
        lines.extend(render_grid_row(row, &widths, &table.alignments));
        if row_idx + 1 == table.header_row_count {
            lines.push(Line::from(
                widths
                    .iter()
                    .map(|width| "-".repeat(*width))
                    .collect::<Vec<_>>()
                    .join(" | "),
            ));
        }
    }
    lines
}

fn fit_column_widths(
    ideal: &[usize],
    minimum: &[usize],
    width: Option<usize>,
) -> Option<Vec<usize>> {
    let Some(width) = width else {
        return Some(ideal.to_vec());
    };
    let available = width
        .max(1)
        .checked_sub(ideal.len().saturating_sub(1) * 3)?;
    if minimum.iter().sum::<usize>() > available {
        return None;
    }
    if ideal.iter().sum::<usize>() <= available {
        return Some(ideal.to_vec());
    }

    // Find a common cap without iterating over every byte of a long cell.
    // Short columns retain their natural size; the remaining space goes to
    // larger columns, never shrinking below an indivisible grapheme.
    let mut low = 0;
    let mut high = ideal.iter().copied().max().unwrap_or(1);
    while low < high {
        let cap = low + (high - low).div_ceil(2);
        let used = ideal
            .iter()
            .zip(minimum)
            .map(|(ideal, min)| (*ideal).min(cap).max(*min))
            .sum::<usize>();
        if used <= available {
            low = cap;
        } else {
            high = cap - 1;
        }
    }
    let mut widths = ideal
        .iter()
        .zip(minimum)
        .map(|(ideal, min)| (*ideal).min(low).max(*min))
        .collect::<Vec<_>>();
    let mut remainder = available - widths.iter().sum::<usize>();
    for (width, ideal) in widths.iter_mut().zip(ideal) {
        if remainder > 0 && *width < *ideal {
            *width += 1;
            remainder -= 1;
        }
    }
    Some(widths)
}

fn render_grid_row(
    row: &[Line<'static>],
    widths: &[usize],
    alignments: &[Alignment],
) -> Vec<Line<'static>> {
    let cells = widths
        .iter()
        .enumerate()
        .map(|(idx, width)| {
            let empty = Line::default();
            wrap_line(
                row.get(idx).unwrap_or(&empty),
                u16::try_from(*width).unwrap_or(u16::MAX),
            )
        })
        .collect::<Vec<_>>();
    let height = cells.iter().map(Vec::len).max().unwrap_or(1);
    (0..height)
        .map(|line_idx| {
            let mut spans = Vec::new();
            for (idx, (cell, width)) in cells.iter().zip(widths).enumerate() {
                if idx > 0 {
                    spans.push(Span::raw(" | "));
                }
                let line = cell.get(line_idx).cloned().unwrap_or_default();
                let padding = width.saturating_sub(display_width(&line.to_string()));
                let left = match alignments.get(idx).copied().unwrap_or(Alignment::None) {
                    Alignment::Right => padding,
                    Alignment::Center => padding / 2,
                    Alignment::None | Alignment::Left => 0,
                };
                if left > 0 {
                    spans.push(Span::raw(" ".repeat(left)));
                }
                spans.extend(line.spans);
                if padding > left {
                    spans.push(Span::raw(" ".repeat(padding - left)));
                }
            }
            Line::from(spans)
        })
        .collect()
}

fn render_records(table: &TableRenderState, width: usize) -> Vec<Line<'static>> {
    let width = u16::try_from(width).unwrap_or(u16::MAX);
    let Some(header) = table.rows.first() else {
        return Vec::new();
    };
    if table.rows.len() == table.header_row_count {
        return header
            .iter()
            .flat_map(|cell| wrap_line(cell, width))
            .collect();
    }
    let mut lines = Vec::new();
    for row in table.rows.iter().skip(table.header_row_count) {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        for (idx, cell) in row.iter().enumerate() {
            let mut field = header
                .get(idx)
                .cloned()
                .filter(|cell| !cell.to_string().trim().is_empty())
                .unwrap_or_else(|| Line::from(format!("Column {}", idx + 1)));
            field.push_span(Span::raw(": "));
            field.spans.extend(cell.spans.iter().cloned());
            lines.extend(wrap_line(&field, width));
        }
    }
    lines
}
