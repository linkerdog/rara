//! Immutable styled/text rows, indexed without flattening retained history.

use std::rc::Rc;

use ratatui::text::Line;

use super::{text_wrap::display_width, transcript_text::wrap_lines};

#[derive(Debug)]
pub(crate) struct VisualRow {
    pub line: Line<'static>,
    pub text: String,
    pub width: usize,
}

#[derive(Debug, Default)]
pub(crate) struct RowBlock {
    rows: Vec<VisualRow>,
}

impl RowBlock {
    pub(crate) fn wrap(lines: &[Line<'static>], width: u16) -> Self {
        Self::from_visual_lines(wrap_lines(lines, width))
    }

    pub(crate) fn from_visual_lines(lines: Vec<Line<'static>>) -> Self {
        Self {
            rows: lines
                .into_iter()
                .map(|line| {
                    let text = line.to_string();
                    let width = display_width(&text);
                    VisualRow { line, text, width }
                })
                .collect(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }
}

#[derive(Clone, Debug, Default)]
struct HistoryBlocks {
    blocks: Vec<Rc<RowBlock>>,
    ends: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SharedHistory(Rc<HistoryBlocks>);

impl SharedHistory {
    pub(crate) fn len(&self) -> usize {
        self.0.ends.last().copied().unwrap_or(0)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn append(&mut self, block: Rc<RowBlock>) {
        if block.len() == 0 {
            return;
        }
        let end = self.len() + block.len();
        // Only block handles/index metadata can be copied while a prior frame
        // or selection retains the old collection. Styled/text rows stay shared.
        let history = Rc::make_mut(&mut self.0);
        history.blocks.push(block);
        history.ends.push(end);
    }

    fn get(&self, row: usize) -> Option<&VisualRow> {
        let block = self.0.ends.partition_point(|end| *end <= row);
        let start = if block == 0 {
            0
        } else {
            self.0.ends[block - 1]
        };
        self.0.blocks.get(block)?.rows.get(row - start)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptRows {
    history: SharedHistory,
    active: Rc<RowBlock>,
}

impl TranscriptRows {
    pub(crate) fn new(history: SharedHistory, active: Rc<RowBlock>) -> Self {
        Self { history, active }
    }

    pub(crate) fn len(&self) -> usize {
        self.history.len() + self.active.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn get(&self, row: usize) -> Option<&VisualRow> {
        if row < self.history.len() {
            self.history.get(row)
        } else {
            self.active.rows.get(row - self.history.len())
        }
    }

    #[cfg(test)]
    pub(crate) fn from_visual_lines(lines: Vec<Line<'static>>) -> Self {
        Self::new(
            SharedHistory::default(),
            Rc::new(RowBlock::from_visual_lines(lines)),
        )
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Line<'static>> {
        self.history
            .0
            .blocks
            .iter()
            .flat_map(|block| &block.rows)
            .chain(&self.active.rows)
            .map(|row| &row.line)
    }
}

#[cfg(test)]
impl std::ops::Index<usize> for TranscriptRows {
    type Output = Line<'static>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.get(index).expect("test row index in bounds").line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segmented_index_preserves_boundaries_and_retained_snapshots() {
        let mut history = SharedHistory::default();
        history.append(Rc::new(RowBlock::from_visual_lines(vec![
            Line::from("first"),
            Line::from(""),
        ])));
        let retained = TranscriptRows::new(history.clone(), Rc::default());
        history.append(Rc::default());
        history.append(Rc::new(RowBlock::from_visual_lines(vec![Line::from(
            "second",
        )])));
        let rows = TranscriptRows::new(
            history,
            Rc::new(RowBlock::from_visual_lines(vec![Line::from("tail")])),
        );
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter().map(Line::to_string).collect::<Vec<_>>(),
            ["first", "", "second", "tail"]
        );
        for (index, expected) in ["first", "", "second", "tail"].into_iter().enumerate() {
            assert_eq!(rows.get(index).unwrap().text, expected);
        }
        assert!(rows.get(4).is_none());
        assert!(rows.get(usize::MAX).is_none());
        assert_eq!(retained.len(), 2);
        assert!(std::ptr::eq(rows.get(0).unwrap(), retained.get(0).unwrap()));

        use crate::tui::selection::{ScreenPosition, TranscriptSelection};
        let mut selection = TranscriptSelection::default();
        selection.update_snapshot(&rows, ratatui::layout::Rect::new(0, 0, 10, 4), 0);
        assert!(selection.start(ScreenPosition::new(0, 0)));
        assert!(selection.drag(ScreenPosition::new(4, 3)));
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("first\n\nsecond\ntail")
        );
    }
}
