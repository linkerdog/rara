//! Retain only syntax parser state, not the previously highlighted code.

use ratatui::text::{Line, Span};
use syntect::{
    easy::HighlightLines, highlighting::HighlightState, parsing::ParseState, util::LinesWithEndings,
};

use super::{
    MAX_HIGHLIGHT_BYTES, MAX_HIGHLIGHT_LINES, convert_style, find_syntax, syntax_set,
    syntax_theme_revision, theme_lock,
};

#[derive(Clone)]
pub(crate) struct StreamingCodeHighlighter {
    state: Option<ColoredCode>,
}

#[derive(Clone)]
struct ColoredCode {
    bytes: usize,
    lines: usize,
    theme_revision: u64,
    syntax: (HighlightState, ParseState),
}

impl StreamingCodeHighlighter {
    pub fn plain() -> Self {
        Self { state: None }
    }

    pub fn for_language(code: &str, lang: &str) -> Option<Self> {
        let line_count = code.lines().count();
        let Some(syntax) = find_syntax(lang)
            .filter(|_| code.len() <= MAX_HIGHLIGHT_BYTES && line_count <= MAX_HIGHLIGHT_LINES)
        else {
            return Some(Self::plain());
        };
        let theme = match theme_lock().read() {
            Ok(theme) => theme,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut highlighter = HighlightLines::new(syntax, &theme);
        for line in LinesWithEndings::from(code) {
            if let Err(error) = highlighter.highlight_line(line, syntax_set()) {
                log::warn!("streaming syntax replay failed: {error}");
                return None;
            }
        }
        Some(Self {
            state: Some(ColoredCode {
                bytes: code.len(),
                lines: line_count,
                theme_revision: syntax_theme_revision(),
                syntax: highlighter.state(),
            }),
        })
    }

    pub fn append(&mut self, code: &str) -> Option<Vec<Line<'static>>> {
        if code.is_empty() {
            return Some(Vec::new());
        }
        let Some(mut state) = self.state.take() else {
            return Some(
                code.lines()
                    .map(|line| Line::from(line.to_string()))
                    .collect(),
            );
        };
        let bytes = state.bytes.checked_add(code.len())?;
        let lines = state.lines.checked_add(code.lines().count())?;
        if bytes > MAX_HIGHLIGHT_BYTES || lines > MAX_HIGHLIGHT_LINES {
            // A canonical replay removes old colors when the aggregate limit is crossed.
            return None;
        }
        let theme = match theme_lock().read() {
            Ok(theme) => theme,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.theme_revision != syntax_theme_revision() {
            return None;
        }
        let (highlight_state, parse_state) = state.syntax;
        let mut highlighter = HighlightLines::from_state(&theme, highlight_state, parse_state);
        let mut rendered = Vec::new();
        for line in LinesWithEndings::from(code) {
            let ranges = match highlighter.highlight_line(line, syntax_set()) {
                Ok(ranges) => ranges,
                Err(error) => {
                    log::warn!("streaming syntax append failed: {error}");
                    return None;
                }
            };
            let mut spans = Vec::new();
            for (style, text) in ranges {
                let text = text.trim_end_matches(['\n', '\r']);
                if !text.is_empty() {
                    spans.push(Span::styled(text.to_string(), convert_style(style)));
                }
            }
            if spans.is_empty() {
                spans.push(Span::raw(String::new()));
            }
            rendered.push(Line::from(spans));
        }
        state.bytes = bytes;
        state.lines = lines;
        state.syntax = highlighter.state();
        self.state = Some(state);
        Some(rendered)
    }
}
