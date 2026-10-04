//! One append-only source and one materialized row cache per live stream.
mod code_fence;

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use code_fence::OpenCodeFence;
use ratatui::text::Line;

#[cfg(test)]
use crate::tui::markdown_render::render_markdown_text_with_width_and_cwd;
use crate::tui::markdown_render::{
    ReferenceBudget, ReferenceContext, RenderContext, render_streaming_markdown,
};
use crate::tui::theme::{self, ThemeRevision};

pub(crate) struct RenderedStream<'a> {
    pub epoch: u64,
    pub revision: usize,
    pub stable_lines: usize,
    pub lines: &'a [Line<'static>],
}

#[derive(Clone, Copy)]
enum RenderBoundary {
    CompleteLines,
    Preview,
}

pub(crate) struct MarkdownStreamCollector {
    buffer: String,
    complete_source_len: usize,
    rendered_source_len: usize,
    rendered_complete_len: usize,
    stable_source_len: usize,
    stable_line_len: usize,
    stable_context: RenderContext,
    held_table_start: Option<usize>,
    references: ReferenceContext,
    reference_replay: bool,
    closing_brackets: usize,
    open_fence: Option<OpenCodeFence>,
    width: Option<usize>,
    cwd: PathBuf,
    lines: Vec<Line<'static>>,
    row_epoch: u64,
    theme_revision: ThemeRevision,
    #[cfg(test)]
    work: MarkdownWork,
}

#[cfg(test)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct MarkdownWork {
    pub parses: usize,
    pub parsed_bytes: usize,
    pub appended_bytes: usize,
    pub rendered_rows: usize,
    pub fence_bytes: usize,
    pub reference_bytes: usize,
}

impl MarkdownStreamCollector {
    pub fn new(width: Option<usize>, cwd: &Path) -> Self {
        Self {
            buffer: String::new(),
            complete_source_len: 0,
            rendered_source_len: 0,
            rendered_complete_len: 0,
            stable_source_len: 0,
            stable_line_len: 0,
            stable_context: RenderContext::default(),
            held_table_start: None,
            references: ReferenceContext::default(),
            reference_replay: false,
            closing_brackets: 0,
            open_fence: None,
            width,
            cwd: cwd.to_path_buf(),
            lines: Vec::new(),
            row_epoch: 0,
            theme_revision: theme::revision(),
            #[cfg(test)]
            work: MarkdownWork::default(),
        }
    }

    pub fn push_delta(&mut self, delta: &str) {
        self.closing_brackets += delta.bytes().filter(|&byte| byte == b']').count();
        if let Some(newline) = delta.rfind('\n') {
            self.complete_source_len = self.buffer.len() + newline + 1;
        }
        self.buffer.push_str(delta);
        #[cfg(test)]
        {
            self.work.appended_bytes += delta.len();
            self.work.reference_bytes += delta.len();
        }
    }

    pub fn replace_source(&mut self, source: &str) {
        self.buffer.clear();
        self.complete_source_len = 0;
        self.closing_brackets = 0;
        self.reset_render();
        self.push_delta(source);
    }

    fn reset_render(&mut self) {
        self.row_epoch = self.row_epoch.wrapping_add(1);
        self.rendered_source_len = 0;
        self.rendered_complete_len = 0;
        self.stable_source_len = 0;
        self.stable_line_len = 0;
        self.stable_context = RenderContext::default();
        self.held_table_start = None;
        self.references = ReferenceContext::default();
        self.reference_replay = false;
        self.open_fence = None;
        self.lines.clear();
    }

    pub fn lines(&mut self) -> &[Line<'static>] {
        let theme_revision = theme::revision();
        if self.theme_revision != theme_revision {
            self.reset_render();
            self.theme_revision = theme_revision;
        }
        if self.needs_render() {
            self.refresh();
            self.rendered_source_len = self.buffer.len();
        }
        &self.lines
    }

    pub fn cached_lines(&self) -> &[Line<'static>] {
        &self.lines
    }

    pub fn needs_render(&self) -> bool {
        self.rendered_source_len != self.buffer.len() || self.theme_revision != theme::revision()
    }

    /// Describe already-materialized rows without coupling source and layout borrows.
    pub(crate) fn rendered_stream(&self) -> RenderedStream<'_> {
        debug_assert!(
            !self.needs_render(),
            "materialize source before reading row boundaries"
        );
        let stable_lines = self
            .open_fence
            .as_ref()
            .map_or(self.stable_line_len, |fence| fence.row_end)
            .min(self.lines.len());
        RenderedStream {
            epoch: self.row_epoch,
            revision: self.rendered_source_len,
            stable_lines,
            lines: &self.lines,
        }
    }

    fn refresh(&mut self) {
        if self.held_table_start.is_some() {
            return;
        }
        if !self.references.allows_incremental(ReferenceBudget {
            source_bytes: self.buffer.len(),
            closing_brackets: self.closing_brackets,
        }) {
            self.reference_replay = true;
        }
        if self.reference_replay
            || self
                .references
                .has_mutable_definition(self.stable_source_len)
        {
            self.invalidate_reference_prefix();
        }
        if let Some(mut fence) = self.open_fence.take() {
            if let Some(rows) = fence.update(&self.buffer, self.complete_source_len) {
                self.lines.truncate(fence.row_end);
                self.record_rows(rows.complete.len() + rows.preview.len());
                #[cfg(test)]
                {
                    self.work.fence_bytes += rows.examined_bytes;
                }
                #[cfg(not(test))]
                let _ = rows.examined_bytes;
                self.lines.extend(rows.complete);
                fence.row_end = self.lines.len();
                self.lines.extend(rows.preview);
                self.open_fence = Some(fence);
                self.rendered_complete_len = self.complete_source_len;
                return;
            }
            // Canonical replay may restyle or normalize already displayed code.
            self.row_epoch = self.row_epoch.wrapping_add(1);
        }
        let new_complete_source = self.complete_source_len > self.rendered_complete_len;
        let render_end = if new_complete_source {
            self.complete_source_len
        } else {
            self.buffer.len()
        };
        let boundary = if new_complete_source {
            RenderBoundary::CompleteLines
        } else {
            RenderBoundary::Preview
        };
        self.render_tail(render_end, boundary);
        self.rendered_complete_len = self.complete_source_len;
        if new_complete_source && self.held_table_start.is_none() && !self.reference_replay {
            let fence_source = &self.buffer[self.stable_source_len..render_end];
            #[cfg(test)]
            {
                self.work.fence_bytes += fence_source.len();
            }
            self.open_fence = OpenCodeFence::detect(fence_source, render_end, self.lines.len());
        }
        if render_end < self.buffer.len() && self.held_table_start.is_none() {
            if let Some(mut fence) = self.open_fence.take()
                && let Some(rows) = fence.update(&self.buffer, self.complete_source_len)
            {
                self.record_rows(rows.preview.len());
                #[cfg(test)]
                {
                    self.work.fence_bytes += rows.examined_bytes;
                }
                #[cfg(not(test))]
                let _ = rows.examined_bytes;
                self.lines.extend(rows.preview);
                self.open_fence = Some(fence);
            } else {
                self.render_tail(self.buffer.len(), RenderBoundary::Preview);
            }
        }
    }

    fn render_tail(&mut self, source_end: usize, boundary: RenderBoundary) {
        let start = self.stable_source_len;
        if start > 0 && !self.reference_source_fits(start..source_end) {
            self.render_reference_replay(source_end, boundary);
            return;
        }
        self.record_parse(source_end - start);
        let pending = render_streaming_markdown(
            &self.buffer[start..source_end],
            self.width,
            &self.cwd,
            self.stable_context,
            &self.references,
        );
        self.record_rows(pending.lines.len());
        if !pending.references.is_empty() && start > 0 {
            self.invalidate_reference_prefix();
            self.render_tail(source_end, boundary);
            return;
        }
        if start == 0 {
            self.references = pending.references;
            if !self.references.allows_incremental(ReferenceBudget {
                source_bytes: self.buffer.len(),
                closing_brackets: self.closing_brackets,
            }) || !self.reference_source_fits(0..source_end)
            {
                self.reference_replay = true;
            }
        }
        if let Some(table_start) = pending.first_table_start
            && matches!(boundary, RenderBoundary::CompleteLines)
        {
            self.held_table_start = Some(start + table_start);
            // The table and following source remain hidden until finalization.
            // Re-render only the preceding mutable blocks, never retained rows.
            if start == 0 {
                // Definitions in the hidden suffix must not revise this prefix.
                self.references = ReferenceContext::default();
            }
            self.render_tail(start + table_start, boundary);
            return;
        }
        if self.reference_replay {
            self.lines = pending.lines;
            return;
        }
        if matches!(boundary, RenderBoundary::CompleteLines)
            && let Some(block_start) = pending.last_block_start
            && !self.reference_source_fits(start..start + block_start)
        {
            self.render_reference_replay(source_end, boundary);
            return;
        }
        let stable_rows = match boundary {
            RenderBoundary::CompleteLines => pending.last_block_start.map(|block_start| {
                self.record_parse(block_start);
                let stable = render_streaming_markdown(
                    &self.buffer[start..start + block_start],
                    self.width,
                    &self.cwd,
                    self.stable_context,
                    &self.references,
                );
                self.record_rows(stable.lines.len());
                (block_start, stable.lines.len(), stable.end_context)
            }),
            RenderBoundary::Preview => None,
        };
        self.lines.truncate(self.stable_line_len);
        self.lines.extend(pending.lines);
        if let Some((source_bytes, row_count, context)) = stable_rows {
            self.stable_source_len += source_bytes;
            self.stable_line_len += row_count;
            self.stable_context = context;
        }
    }

    fn reference_source_fits(&mut self, range: Range<usize>) -> bool {
        if self.references.is_empty() {
            return true;
        }
        #[cfg(test)]
        {
            self.work.reference_bytes += range.len();
        }
        let source = &self.buffer[range];
        self.references.allows_incremental(ReferenceBudget {
            source_bytes: source.len(),
            closing_brackets: source.bytes().filter(|&byte| byte == b']').count(),
        })
    }

    fn render_reference_replay(&mut self, source_end: usize, boundary: RenderBoundary) {
        self.reference_replay = true;
        self.invalidate_reference_prefix();
        self.render_tail(source_end, boundary);
    }

    /// Definitions in a mutable suffix can revise links in retained rows.
    fn invalidate_reference_prefix(&mut self) {
        self.row_epoch = self.row_epoch.wrapping_add(1);
        self.stable_source_len = 0;
        self.stable_line_len = 0;
        self.stable_context = RenderContext::default();
        self.references = ReferenceContext::default();
        self.open_fence = None;
    }

    #[cfg(test)]
    pub fn finalize(&mut self) {
        self.row_epoch = self.row_epoch.wrapping_add(1);
        self.theme_revision = theme::revision();
        self.open_fence = None;
        self.record_parse(self.buffer.len());
        self.lines =
            render_markdown_text_with_width_and_cwd(&self.buffer, self.width, Some(&self.cwd))
                .lines;
        self.record_rows(self.lines.len());
        self.rendered_source_len = self.buffer.len();
        self.rendered_complete_len = self.complete_source_len;
        self.stable_source_len = self.buffer.len();
        self.stable_line_len = self.lines.len();
        self.held_table_start = None;
    }

    fn record_parse(&mut self, bytes: usize) {
        #[cfg(test)]
        {
            self.work.parses += 1;
            self.work.parsed_bytes += bytes;
        }
        #[cfg(not(test))]
        let _ = bytes;
    }

    fn record_rows(&mut self, rows: usize) {
        #[cfg(test)]
        {
            self.work.rendered_rows += rows;
        }
        #[cfg(not(test))]
        let _ = rows;
    }

    #[cfg(test)]
    pub fn work(&self) -> MarkdownWork {
        self.work
    }
}

#[cfg(test)]
#[path = "markdown_stream_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "markdown_stream_render_tests.rs"]
mod render_tests;
