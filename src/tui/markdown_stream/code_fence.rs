//! A conservative fast path for an open, unindented top-level fence.

use ratatui::text::{Line, Span};

use crate::tui::highlight::StreamingCodeHighlighter;

pub(super) struct OpenCodeFence {
    marker: u8,
    marker_len: usize,
    source_end: usize,
    pub row_end: usize,
    highlighter: StreamingCodeHighlighter,
}

pub(super) struct FenceRows {
    pub complete: Vec<Line<'static>>,
    pub preview: Vec<Line<'static>>,
    pub examined_bytes: usize,
}

impl OpenCodeFence {
    pub fn detect(source: &str, source_end: usize, row_end: usize) -> Option<Self> {
        let marker = *source.as_bytes().first()?;
        if !matches!(marker, b'`' | b'~') || source.contains(['\r', '\0']) {
            return None;
        }
        let (opening, code) = source.split_once('\n')?;
        let marker_len = opening.bytes().take_while(|byte| *byte == marker).count();
        let info = opening[marker_len..].trim_matches([' ', '\t', '\u{b}', '\u{c}']);
        if marker_len < 3
            || code.is_empty()
            || !code.ends_with('\n')
            || info.contains(['&', '\\'])
            || (marker == b'`' && info.contains('`'))
            || possible_closer(code, marker, marker_len)
        {
            return None;
        }
        let language = info
            .split([',', ' ', '\t'])
            .next()
            .filter(|lang| !lang.is_empty());
        let highlighter = match language {
            Some(language) => StreamingCodeHighlighter::for_language(code, language)?,
            None => StreamingCodeHighlighter::plain(),
        };
        Some(Self {
            marker,
            marker_len,
            source_end,
            row_end,
            highlighter,
        })
    }

    pub fn update(&mut self, source: &str, complete_end: usize) -> Option<FenceRows> {
        let complete = source.get(self.source_end..complete_end)?;
        let partial = source.get(complete_end..)?;
        if complete.contains(['\r', '\0'])
            || partial.contains(['\r', '\0'])
            || possible_closer(complete, self.marker, self.marker_len)
            || possible_closer(partial, self.marker, self.marker_len)
        {
            return None;
        }
        let complete_rows = self.highlighter.append(complete)?;
        // Preview cannot advance syntax state: completing this line replaces it.
        let preview_rows = self.highlighter.clone().append(partial)?;
        self.source_end = complete_end;
        Some(FenceRows {
            complete: with_writer_indent(complete_rows),
            preview: with_writer_indent(preview_rows),
            examined_bytes: complete.len() + partial.len(),
        })
    }
}

fn with_writer_indent(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, Span::default());
            line
        })
        .collect()
}

fn possible_closer(source: &str, marker: u8, marker_len: usize) -> bool {
    source.lines().any(|line| {
        line.trim_start_matches([' ', '\t'])
            .bytes()
            .take_while(|byte| *byte == marker)
            .count()
            >= marker_len
    })
}
