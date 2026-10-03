//! Canonical table collection and width-aware rendering.

use pulldown_cmark::Alignment;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug)]
pub(super) struct TableRenderState {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<String>>,
    current_row: Option<Vec<String>>,
    current_cell: Option<String>,
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
        self.current_cell = Some(String::new());
    }

    pub(super) fn end_cell(&mut self) {
        let cell = self.current_cell.take().unwrap_or_default();
        if let Some(row) = self.current_row.as_mut() {
            row.push(normalize_table_cell(&cell));
        }
    }

    pub(super) fn push_text(&mut self, text: &str) {
        if let Some(cell) = self.current_cell.as_mut() {
            cell.push_str(text);
        }
    }
}

fn normalize_table_cell(cell: &str) -> String {
    cell.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn render_table_lines(table: &TableRenderState, width: Option<usize>) -> Vec<String> {
    if table.rows.is_empty() {
        return Vec::new();
    }

    let column_count = table.rows.iter().map(Vec::len).max().unwrap_or(0);
    if column_count == 0 {
        return Vec::new();
    }

    let mut column_widths = vec![1usize; column_count];
    for row in &table.rows {
        for (idx, cell) in row.iter().enumerate() {
            column_widths[idx] = column_widths[idx].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }
    fit_table_width(&mut column_widths, width);

    let mut lines = Vec::new();
    for (row_idx, row) in table.rows.iter().enumerate() {
        lines.push(render_table_row(row, &column_widths, &table.alignments));
        if row_idx + 1 == table.header_row_count {
            lines.push(render_table_separator(&column_widths, &table.alignments));
        }
    }
    lines
}

fn fit_table_width(column_widths: &mut [usize], width: Option<usize>) {
    let Some(max_width) = width else {
        return;
    };
    if column_widths.is_empty() {
        return;
    }

    let separator_width = column_widths.len().saturating_sub(1) * 3;
    let total_width = column_widths.iter().sum::<usize>() + separator_width;
    if total_width <= max_width {
        return;
    }

    let available_cells = max_width
        .saturating_sub(separator_width)
        .max(column_widths.len());
    let max_column_width = (available_cells / column_widths.len()).max(1);
    for width in column_widths {
        *width = (*width).min(max_column_width).max(1);
    }
}

fn render_table_row(row: &[String], column_widths: &[usize], alignments: &[Alignment]) -> String {
    column_widths
        .iter()
        .enumerate()
        .map(|(idx, width)| {
            let cell = row.get(idx).map(String::as_str).unwrap_or("");
            pad_table_cell(
                truncate_to_width(cell, *width).as_str(),
                *width,
                alignment_for_column(alignments, idx),
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn render_table_separator(column_widths: &[usize], alignments: &[Alignment]) -> String {
    column_widths
        .iter()
        .enumerate()
        .map(|(idx, width)| {
            let dashes = "-".repeat(*width);
            match alignment_for_column(alignments, idx) {
                Alignment::Left | Alignment::Center | Alignment::Right | Alignment::None => dashes,
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn alignment_for_column(alignments: &[Alignment], idx: usize) -> Alignment {
    alignments.get(idx).copied().unwrap_or(Alignment::None)
}

fn pad_table_cell(cell: &str, width: usize, alignment: Alignment) -> String {
    let cell_width = UnicodeWidthStr::width(cell);
    let padding = width.saturating_sub(cell_width);
    match alignment {
        Alignment::Right => format!("{}{cell}", " ".repeat(padding)),
        Alignment::Center => {
            let left = padding / 2;
            let right = padding - left;
            format!("{}{cell}{}", " ".repeat(left), " ".repeat(right))
        }
        Alignment::Left | Alignment::None => format!("{cell}{}", " ".repeat(padding)),
    }
}

fn truncate_to_width(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    let ellipsis_width = 1usize;
    for ch in value.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + ellipsis_width > max_width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    out
}
