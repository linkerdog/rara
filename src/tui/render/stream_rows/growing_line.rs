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
        let [body] = line.spans.as_slice() else {
            return None;
        };
        let chrome = RespondingCell::stream_prefix(self.index).concat();
        let prefix = chrome.get(self.offset.min(chrome.len())..)?;
        let content = body
            .content
            .get(self.offset.saturating_sub(chrome.len())..)?;
        let input = Line::from(vec![Span::raw(prefix), Span::styled(content, body.style)])
            .style(line.style);
        #[cfg(test)]
        {
            work.record(WorkKind::Wrap, 1);
            work.record(WorkKind::WrapBytes, prefix.len() + content.len());
        }
        let mut wrapped = wrap_line_with_source(&input, width);
        #[cfg(test)]
        work.record(WorkKind::Text, wrapped.lines.len());
        // Ranges are in sanitized text. A changed projection requires replay
        // from the saved logical-line boundary, not offsets into raw source.
        if wrapped.source.len() != prefix.len() + content.len()
            || !wrapped.source.starts_with(prefix)
            || wrapped.source.get(prefix.len()..) != Some(content)
        {
            return None;
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
