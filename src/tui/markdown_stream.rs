//! One append-only source and one materialized row cache per live stream.
mod code_fence;

use std::path::{Path, PathBuf};

use code_fence::OpenCodeFence;
use ratatui::text::Line;

use crate::tui::markdown_render::{
    RenderContext, render_markdown_text_with_width_and_cwd, render_streaming_markdown,
};
use crate::tui::render::{RenderedStream, ResponseView, StreamRowCache};
use crate::tui::transcript_rows::TranscriptRows;

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
    has_references: bool,
    open_fence: Option<OpenCodeFence>,
    width: Option<usize>,
    cwd: PathBuf,
    lines: Vec<Line<'static>>,
    row_epoch: u64,
    response_layout: StreamRowCache,
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
            has_references: false,
            open_fence: None,
            width,
            cwd: cwd.to_path_buf(),
            lines: Vec::new(),
            row_epoch: 0,
            response_layout: StreamRowCache::default(),
            #[cfg(test)]
            work: MarkdownWork::default(),
        }
    }

    pub fn push_delta(&mut self, delta: &str) {
        if let Some(newline) = delta.rfind('\n') {
            self.complete_source_len = self.buffer.len() + newline + 1;
        }
        self.buffer.push_str(delta);
        #[cfg(test)]
        {
            self.work.appended_bytes += delta.len();
        }
    }

    pub fn replace_source(&mut self, source: &str) {
        self.buffer.clear();
        self.complete_source_len = 0;
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
        self.has_references = false;
        self.open_fence = None;
        self.lines.clear();
    }

    pub fn lines(&mut self) -> &[Line<'static>] {
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
        self.rendered_source_len != self.buffer.len()
    }

    pub(crate) fn response_rows(&mut self, width: u16, view: ResponseView) -> TranscriptRows {
        self.lines();
        let stable_lines = self
            .open_fence
            .as_ref()
            .map_or(self.stable_line_len, |fence| fence.row_end)
            .min(self.lines.len());
        self.response_layout.materialize(
            RenderedStream {
                epoch: self.row_epoch,
                revision: self.rendered_source_len,
                stable_lines,
                lines: &self.lines,
            },
            width,
            view,
        )
    }

    fn refresh(&mut self) {
        if self.held_table_start.is_some() {
            return;
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
        if new_complete_source && self.held_table_start.is_none() && !self.has_references {
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
        self.record_parse(source_end - start);
        let pending = render_streaming_markdown(
            &self.buffer[start..source_end],
            self.width,
            &self.cwd,
            self.stable_context,
        );
        self.record_rows(pending.lines.len());
        if pending.has_references && !self.has_references {
            self.row_epoch = self.row_epoch.wrapping_add(1);
            self.has_references = true;
            self.stable_source_len = 0;
            self.stable_line_len = 0;
            self.stable_context = RenderContext::default();
            self.render_tail(source_end, boundary);
            return;
        }
        if let Some(table_start) = pending.first_table_start
            && matches!(boundary, RenderBoundary::CompleteLines)
        {
            self.held_table_start = Some(start + table_start);
            // The table and following source remain hidden until finalization.
            // Re-render only the preceding mutable blocks, never retained rows.
            self.render_tail(start + table_start, boundary);
            return;
        }
        if self.has_references {
            self.lines = pending.lines;
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

    pub fn finalize(&mut self) {
        self.row_epoch = self.row_epoch.wrapping_add(1);
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

    #[cfg(test)]
    pub(crate) fn layout_work(&self) -> crate::tui::transcript_work::WorkMeter {
        self.response_layout.work.clone()
    }
}

#[cfg(test)]
#[path = "markdown_stream_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "markdown_stream_render_tests.rs"]
mod render_tests;
