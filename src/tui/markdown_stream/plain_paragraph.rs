//! Extend canonical plain paragraph rows until appended syntax requires replay.

use ratatui::text::{Line, Span};

pub(super) struct PlainParagraph {
    at_line_start: bool,
    pending_spaces: usize,
    pub row_start: usize,
    pub row_end: usize,
}

impl PlainParagraph {
    pub fn from_rendered(source: &str, lines: &[Line<'static>]) -> Option<Self> {
        if source.is_empty() {
            return None;
        }
        let mut plain = Self {
            at_line_start: true,
            pending_spaces: 0,
            row_start: 0,
            row_end: 0,
        };
        plain.validate(source)?;
        plain.row_start = lines.len().checked_sub(source.lines().count())?;
        plain.row_end = lines.len().checked_sub(usize::from(!plain.at_line_start))?;
        Some(plain)
    }

    /// Validate before mutating rows, so fallback sees the original cache.
    pub fn append(&mut self, delta: &str, lines: &mut Vec<Line<'static>>) -> Option<usize> {
        let mut at_line_start = self.at_line_start;
        let mut pending_spaces = self.pending_spaces;
        self.validate(delta)?;
        let mut changed_rows = 0;
        for part in delta.split_inclusive('\n') {
            let text = part.trim_end_matches([' ', '\n']);
            if at_line_start {
                lines.push(Line::from(Span::raw(String::new())));
            }
            let content = lines.last_mut()?.spans.first_mut()?.content.to_mut();
            if !text.is_empty() {
                content.extend(std::iter::repeat_n(' ', pending_spaces));
                content.push_str(text);
                pending_spaces = 0;
            }
            at_line_start = part.ends_with('\n');
            pending_spaces = if at_line_start {
                0
            } else {
                pending_spaces + part.len() - text.len()
            };
            changed_rows += 1;
        }
        self.row_end = lines.len() - usize::from(!self.at_line_start);
        Some(changed_rows)
    }

    fn validate(&mut self, source: &str) -> Option<()> {
        for ch in source.chars() {
            if ch == '\u{feff}' {
                // The parser consumes an initial BOM instead of displaying it.
                return None;
            }
            if self.at_line_start {
                if !(ch.is_ascii_alphabetic()
                    || (!ch.is_ascii() && !ch.is_whitespace() && !ch.is_control()))
                {
                    return None;
                }
                self.at_line_start = false;
            } else if ch == '\n' {
                // A hard break at EOF can create an additional canonical row.
                if self.pending_spaces >= 2 {
                    return None;
                }
                self.at_line_start = true;
            } else if ch.is_control()
                || (ch.is_whitespace() && ch != ' ')
                || matches!(
                    ch,
                    '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '&' | '~' | '|'
                )
            {
                return None;
            }
            self.pending_spaces = if ch == ' ' {
                self.pending_spaces + 1
            } else {
                0
            };
        }
        Some(())
    }
}
