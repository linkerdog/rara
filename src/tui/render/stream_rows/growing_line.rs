//! Retain visual rows before the mutable end of one append-only logical line.

use ratatui::text::{Line, Span};

use super::RespondingCell;
use crate::tui::transcript_rows::{RowBlock, SharedHistory};
use crate::tui::transcript_text::wrap_line_with_source;
#[cfg(test)]
use crate::tui::transcript_work::{WorkKind, WorkMeter};

#[derive(Clone, Copy)]
pub(super) enum LineBoundary {
    Preview,
    Complete,
}

pub(super) struct GrowingLine {
    pub index: usize,
    pub before: SharedHistory,
    pub rejected: bool,
    offset: usize,
    span_index: usize,
    span_start: usize,
}

pub(super) struct LineRows {
    pub stable: RowBlock,
    pub tail: RowBlock,
}

impl GrowingLine {
    pub fn new(index: usize, before: SharedHistory) -> Self {
        Self {
            index,
            before,
            rejected: false,
            offset: 0,
            span_index: 0,
            span_start: 0,
        }
    }

    pub fn render(
        &mut self,
        line: &Line<'static>,
        width: u16,
        boundary: LineBoundary,
        #[cfg(test)] work: &WorkMeter,
    ) -> Option<LineRows> {
        if self.rejected {
            return None;
        }
        let chrome = RespondingCell::stream_prefix(self.index).concat();
        let prefix = chrome.get(self.offset.min(chrome.len())..)?;
        let body_offset = self.offset.saturating_sub(chrome.len());
        // Earlier spans cannot change within the collector's append-only epoch.
        // Keep the last span even at its end: later deltas can extend it.
        while self.span_index + 1 < line.spans.len() {
            let end = self.span_start + line.spans[self.span_index].content.len();
            if body_offset < end {
                break;
            }
            self.span_start = end;
            self.span_index += 1;
            #[cfg(test)]
            work.record(WorkKind::StreamSpans, 1);
        }
        let mut spans = vec![Span::raw(prefix)];
        for (index, span) in line.spans.get(self.span_index..)?.iter().enumerate() {
            let start = if index == 0 {
                body_offset - self.span_start
            } else {
                0
            };
            spans.push(Span::styled(span.content.get(start..)?, span.style));
        }
        let input = Line::from(spans).style(line.style);
        let input_bytes = input
            .spans
            .iter()
            .map(|span| span.content.len())
            .sum::<usize>();
        #[cfg(test)]
        {
            work.record(WorkKind::Wrap, 1);
            work.record(WorkKind::WrapBytes, input_bytes);
            work.record(WorkKind::StreamSpans, input.spans.len());
        }
        let mut wrapped = wrap_line_with_source(&input, width);
        #[cfg(test)]
        work.record(WorkKind::Text, wrapped.lines.len());
        // Ranges are in sanitized text. A changed projection requires replay
        // from the saved logical-line boundary, not offsets into raw source.
        if wrapped.source.len() != input_bytes {
            return None;
        }
        let mut remaining = wrapped.source.as_str();
        for span in &input.spans {
            remaining = remaining.strip_prefix(span.content.as_ref())?;
        }
        let retained = match boundary {
            // Keep the last grapheme, the preceding word fragment, and the row
            // that fragment can move back into if the grapheme's width shrinks.
            LineBoundary::Preview => wrapped.lines.len().saturating_sub(3),
            LineBoundary::Complete => wrapped.lines.len(),
        };
        self.offset += wrapped
            .ranges
            .get(retained)
            .map_or(wrapped.source.len(), |range| range.start);
        let tail = wrapped.lines.split_off(retained);
        Some(LineRows {
            stable: RowBlock::from_visual_lines(wrapped.lines),
            tail: RowBlock::from_visual_lines(tail),
        })
    }
}
