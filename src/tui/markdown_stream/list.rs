//! Replace only the final root-list item while preserving prior item rows.

use super::MarkdownStreamCollector;
use crate::tui::markdown_render::{ListTail, render_list_continuation};

impl MarkdownStreamCollector {
    pub(super) fn refresh_list(&mut self, previous: ListTail) -> bool {
        if !self.reference_source_fits(previous.source_start..self.buffer.len()) {
            return false;
        }
        let Some(pending) = render_list_continuation(
            &self.buffer[previous.source_start..],
            self.width,
            &self.cwd,
            &self.references,
            &previous,
        ) else {
            return false;
        };
        self.record_parse(pending.parsed_bytes);
        #[cfg(test)]
        {
            self.work.list_seed_bytes += pending.seed_bytes;
        }
        self.record_rows(pending.lines.len());
        let Some(mut tail) = pending.list_tail else {
            return false;
        };
        if tail.root_start != 0
            || pending.last_block_start.is_some()
            || pending.first_table_start.is_some()
            || !pending.references.is_empty()
            || tail.index.is_some() != previous.index.is_some()
            || (tail.loose && !previous.loose)
        {
            return false;
        }
        tail.loose |= previous.loose;
        tail.shift(previous.source_start, previous.row_start);
        self.lines.truncate(previous.row_start);
        self.lines.extend(pending.lines);
        self.list_tail = Some(tail);
        true
    }
}
