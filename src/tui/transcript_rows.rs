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

#[derive(Debug)]
enum HistoryNode {
    Block(Rc<RowBlock>),
    Pair {
        left: Rc<HistoryTree>,
        right: Rc<HistoryTree>,
    },
}

#[derive(Debug)]
struct HistoryTree {
    rows: usize,
    blocks: usize,
    node: HistoryNode,
}

impl HistoryTree {
    fn get(&self, row: usize) -> Option<&VisualRow> {
        match &self.node {
            HistoryNode::Block(block) => block.rows.get(row),
            HistoryNode::Pair { left, right } => {
                if row < left.rows {
                    left.get(row)
                } else {
                    right.get(row - left.rows)
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SharedHistory {
    // A binary-carry forest has at most 1 + floor(log2(blocks)) roots. Snapshots
    // copy only this small root list; immutable balanced subtrees stay shared.
    roots: Rc<Vec<Rc<HistoryTree>>>,
    rows: usize,
}

impl SharedHistory {
    pub(crate) fn len(&self) -> usize {
        self.rows
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn append(&mut self, block: Rc<RowBlock>) {
        if block.len() == 0 {
            return;
        }
        self.rows += block.len();
        let roots = Rc::make_mut(&mut self.roots);
        let mut tree = Rc::new(HistoryTree {
            rows: block.len(),
            blocks: 1,
            node: HistoryNode::Block(block),
        });
        while roots.last().is_some_and(|root| root.blocks == tree.blocks) {
            let Some(left) = roots.pop() else {
                break;
            };
            tree = Rc::new(HistoryTree {
                rows: left.rows + tree.rows,
                blocks: left.blocks + tree.blocks,
                node: HistoryNode::Pair { left, right: tree },
            });
        }
        roots.push(tree);
    }

    fn get(&self, mut row: usize) -> Option<&VisualRow> {
        for root in self.roots.iter() {
            if row < root.rows {
                return root.get(row);
            }
            row -= root.rows;
        }
        None
    }
}

#[derive(Debug)]
enum RowSource {
    Blocks {
        history: SharedHistory,
        active: Rc<RowBlock>,
    },
    Joined {
        first: TranscriptRows,
        second: TranscriptRows,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptRows {
    source: Rc<RowSource>,
    len: usize,
}

impl Default for TranscriptRows {
    fn default() -> Self {
        Self::new(SharedHistory::default(), Rc::default())
    }
}

impl TranscriptRows {
    pub(crate) fn new(history: SharedHistory, active: Rc<RowBlock>) -> Self {
        Self {
            len: history.len() + active.len(),
            source: Rc::new(RowSource::Blocks { history, active }),
        }
    }

    pub(crate) fn joined(first: Self, second: Self) -> Self {
        if first.is_empty() {
            return second;
        }
        if second.is_empty() {
            return first;
        }
        Self {
            len: first.len() + second.len(),
            source: Rc::new(RowSource::Joined { first, second }),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn get(&self, row: usize) -> Option<&VisualRow> {
        match self.source.as_ref() {
            RowSource::Blocks { history, active } => {
                if row < history.len() {
                    history.get(row)
                } else {
                    active.rows.get(row - history.len())
                }
            }
            RowSource::Joined { first, second } => {
                if row < first.len() {
                    first.get(row)
                } else {
                    second.get(row - first.len())
                }
            }
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
        (0..self.len()).filter_map(|row| self.get(row).map(|row| &row.line))
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

    #[test]
    fn retained_forest_snapshots_copy_only_logarithmic_roots() {
        let mut history = SharedHistory::default();
        let mut retained = Vec::new();
        let mut expected = Vec::new();
        for block in 0usize..4096 {
            retained.push(TranscriptRows::new(history.clone(), Rc::default()));
            let lines = (0..1 + block % 3)
                .map(|row| Line::from(format!("block-{block}-row-{row}")))
                .collect::<Vec<_>>();
            expected.extend(lines.iter().cloned());
            history.append(Rc::new(RowBlock::from_visual_lines(lines)));
            assert_eq!(history.len(), expected.len());
            assert_eq!(history.roots.len(), (block + 1).count_ones() as usize);
            assert!(
                history
                    .roots
                    .iter()
                    .all(|root| root.blocks.is_power_of_two())
            );
            assert!(
                history
                    .roots
                    .windows(2)
                    .all(|pair| pair[0].blocks > pair[1].blocks)
            );
        }
        let rows = TranscriptRows::new(history, Rc::default());
        assert_eq!(rows.iter().cloned().collect::<Vec<_>>(), expected);
        for snapshot in retained.into_iter().skip(1) {
            assert!(std::ptr::eq(snapshot.get(0).unwrap(), rows.get(0).unwrap()));
            let last = snapshot.len() - 1;
            assert!(std::ptr::eq(
                snapshot.get(last).unwrap(),
                rows.get(last).unwrap()
            ));
            assert!(snapshot.get(snapshot.len()).is_none());
        }
    }

    #[test]
    fn joined_rows_preserve_offsets_snapshots_and_cross_boundary_copy() {
        use crate::tui::selection::{ScreenPosition, TranscriptSelection};

        let prefix = TranscriptRows::from_visual_lines(vec![Line::from("first"), Line::from("")]);
        let stable = TranscriptRows::from_visual_lines(vec![Line::from("second")]);
        let preview = TranscriptRows::from_visual_lines(vec![Line::from("tail")]);
        let rows = TranscriptRows::joined(
            prefix.clone(),
            TranscriptRows::joined(stable.clone(), preview.clone()),
        );
        assert_eq!(rows.len(), 4);
        assert!(std::ptr::eq(prefix.get(0).unwrap(), rows.get(0).unwrap()));
        assert!(std::ptr::eq(stable.get(0).unwrap(), rows.get(2).unwrap()));
        assert!(std::ptr::eq(preview.get(0).unwrap(), rows.get(3).unwrap()));
        assert!(rows.get(4).is_none());
        assert!(rows.get(usize::MAX).is_none());
        let mut selection = TranscriptSelection::default();
        selection.update_snapshot(&rows, ratatui::layout::Rect::new(0, 0, 10, 4), 0);
        assert!(selection.start(ScreenPosition::new(0, 0)));
        assert!(selection.drag(ScreenPosition::new(4, 3)));
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("first\n\nsecond\ntail")
        );
    }

    #[test]
    fn joining_empty_sources_retains_existing_row_allocations() {
        let rows = TranscriptRows::from_visual_lines(vec![Line::from("retained")]);
        for joined in [
            TranscriptRows::joined(TranscriptRows::default(), rows.clone()),
            TranscriptRows::joined(rows.clone(), TranscriptRows::default()),
        ] {
            assert_eq!(joined.len(), 1);
            assert!(std::ptr::eq(rows.get(0).unwrap(), joined.get(0).unwrap()));
        }
    }
}
