//! Stream-owned visual rows: retained stable blocks and a replaceable preview.
mod growing_line;

use std::rc::Rc;

use growing_line::{GrowingLine, LineBoundary};
use ratatui::text::Line;

use super::RespondingCell;
use crate::tui::markdown_stream::RenderedStream;
use crate::tui::transcript_rows::{RowBlock, SharedHistory, TranscriptRows};
#[cfg(test)]
use crate::tui::transcript_work::{WorkKind, WorkMeter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponseView {
    Full,
    Compact,
}

#[derive(PartialEq, Eq)]
struct LayoutKey {
    epoch: u64,
    width: u16,
    view: ResponseView,
}

#[derive(Default)]
pub(crate) struct StreamRowCache {
    key: Option<LayoutKey>,
    revision: Option<usize>,
    stable_lines: usize,
    stable: SharedHistory,
    tail: Rc<RowBlock>,
    partial: Option<GrowingLine>,
    #[cfg(test)]
    pub(crate) work: WorkMeter,
}

impl StreamRowCache {
    pub(crate) fn materialize(
        &mut self,
        render: RenderedStream<'_>,
        width: u16,
        view: ResponseView,
    ) -> TranscriptRows {
        let key = LayoutKey {
            epoch: render.epoch,
            width,
            view,
        };
        let cap = match view {
            ResponseView::Full => render.lines.len(),
            ResponseView::Compact => render.lines.len().min(4),
        };
        let stable_end = render.stable_lines.min(cap);
        if self.key.as_ref() != Some(&key) || stable_end < self.stable_lines {
            self.key = Some(key);
            self.revision = None;
            self.stable_lines = 0;
            self.stable = SharedHistory::default();
            self.partial = None;
        }
        if self.revision != Some(render.revision) {
            if stable_end > self.stable_lines && self.partial.is_some() {
                self.wrap_growing_line(
                    render.lines,
                    self.stable_lines,
                    width,
                    LineBoundary::Complete,
                );
                self.stable_lines += 1;
            }
            if stable_end > self.stable_lines {
                let logical = self.body_lines(render.lines, self.stable_lines..stable_end);
                let block = self.wrap(&logical, width);
                self.stable.append(block);
            }
            if render
                .append_only_start
                .is_some_and(|start| start <= stable_end)
                && stable_end + 1 == cap
                && cap == render.lines.len()
            {
                self.tail =
                    self.wrap_growing_line(render.lines, stable_end, width, LineBoundary::Preview);
            } else {
                let mut logical = self.body_lines(render.lines, stable_end..cap);
                if render.lines.is_empty() {
                    logical.push(Line::from("• "));
                }
                if cap < render.lines.len() {
                    logical.push(RespondingCell::stream_summary_line(
                        render.lines.len() - cap,
                    ));
                }
                self.tail = self.wrap(&logical, width);
            }
            self.stable_lines = stable_end;
            self.revision = Some(render.revision);
        }
        TranscriptRows::new(self.stable.clone(), self.tail.clone())
    }

    fn wrap_growing_line(
        &mut self,
        lines: &[Line<'static>],
        index: usize,
        width: u16,
        boundary: LineBoundary,
    ) -> Rc<RowBlock> {
        let mut partial = self
            .partial
            .take()
            .unwrap_or_else(|| GrowingLine::new(index, self.stable.clone()));
        debug_assert_eq!(partial.index, index);
        let rows = partial.render(
            &lines[index],
            width,
            boundary,
            #[cfg(test)]
            &self.work,
        );
        let tail = match rows {
            Some(rows) => {
                self.stable.append(Rc::new(rows.stable));
                Rc::new(rows.tail)
            }
            None => {
                self.stable = partial.before.clone();
                partial.rejected = true;
                let logical = self.body_lines(lines, index..index + 1);
                let block = self.wrap(&logical, width);
                match boundary {
                    LineBoundary::Preview => block,
                    LineBoundary::Complete => {
                        self.stable.append(block);
                        Rc::default()
                    }
                }
            }
        };
        match boundary {
            LineBoundary::Preview => self.partial = Some(partial),
            LineBoundary::Complete => {}
        }
        tail
    }

    fn body_lines(
        &self,
        lines: &[Line<'static>],
        range: std::ops::Range<usize>,
    ) -> Vec<Line<'static>> {
        #[cfg(test)]
        {
            self.work.record(WorkKind::Clone, range.len());
            self.work.record(
                WorkKind::StreamSpans,
                lines[range.clone()]
                    .iter()
                    .map(|line| line.spans.len())
                    .sum(),
            );
            self.work.record(
                WorkKind::CloneBytes,
                lines[range.clone()]
                    .iter()
                    .flat_map(|line| &line.spans)
                    .map(|span| span.content.len())
                    .sum(),
            );
        }
        range
            .map(|index| RespondingCell::stream_body_line(&lines[index], index))
            .collect()
    }

    fn wrap(&self, lines: &[Line<'static>], width: u16) -> Rc<RowBlock> {
        #[cfg(test)]
        {
            self.work.record(WorkKind::Wrap, lines.len());
            self.work.record(
                WorkKind::WrapBytes,
                lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .map(|span| span.content.len())
                    .sum(),
            );
        }
        let block = Rc::new(RowBlock::wrap(lines, width));
        #[cfg(test)]
        self.work.record(WorkKind::Text, block.len());
        block
    }
}
