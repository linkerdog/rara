//! Bounded UTF-8 display tails, sanitized before line splitting or formatting.

use std::collections::VecDeque;

use super::display_sanitize::StreamSanitizer;

pub(crate) struct TailLimits {
    pub bytes: usize,
    pub lines: usize,
}

#[derive(Debug)]
pub(crate) struct DisplayTail {
    sanitizer: StreamSanitizer,
    tail: VecDeque<char>,
    bytes: usize,
    newlines: usize,
    truncated: bool,
    byte_limit: usize,
    line_limit: usize,
}

impl DisplayTail {
    pub(crate) fn new(limits: TailLimits) -> Self {
        Self {
            sanitizer: StreamSanitizer::default(),
            tail: VecDeque::new(),
            bytes: 0,
            newlines: 0,
            truncated: false,
            byte_limit: limits.bytes.max(1),
            line_limit: limits.lines.max(1),
        }
    }

    pub(crate) fn push_delta(&mut self, chunk: &str) -> bool {
        let mut changed = false;
        self.sanitizer.write(chunk, |ch| {
            changed = true;
            self.tail.push_back(ch);
            self.bytes += ch.len_utf8();
            self.newlines += usize::from(ch == '\n');
            // A trailing LF completes a line, rather than adding an empty row.
            while self.bytes > self.byte_limit
                || self.newlines + usize::from(self.tail.back().is_some_and(|last| *last != '\n'))
                    > self.line_limit
            {
                if let Some(first) = self.tail.pop_front() {
                    self.bytes -= first.len_utf8();
                    self.newlines -= usize::from(first == '\n');
                    self.truncated = true;
                }
            }
        });
        changed
    }

    pub(crate) fn text(&self) -> String {
        self.tail.iter().collect()
    }

    pub(crate) fn is_truncated(&self) -> bool {
        self.truncated
    }

    #[cfg(test)]
    pub(crate) fn retained_size(&self) -> (usize, usize) {
        (self.bytes, self.tail.capacity())
    }
}
