//! Source offsets collected in the canonical writer's parser pass.

use std::{ops::Range, path::Path};

use pulldown_cmark::{Event, Parser, Tag};
use ratatui::text::Line;

use super::{Writer, markdown_options};

/// Root formatting state carried only across completed top-level blocks.
#[derive(Clone, Copy, Default)]
pub(crate) struct RenderContext {
    needs_newline: bool,
    has_output: bool,
}

pub(crate) struct StreamingMarkdown {
    pub lines: Vec<Line<'static>>,
    pub last_block_start: Option<usize>,
    pub first_table_start: Option<usize>,
    pub has_references: bool,
    pub end_context: RenderContext,
}

pub(crate) fn render_streaming_markdown(
    input: &str,
    width: Option<usize>,
    cwd: &Path,
    context: RenderContext,
) -> StreamingMarkdown {
    let parser = Parser::new_ext(input, markdown_options());
    let has_references = parser.reference_definitions().iter().next().is_some();
    let tracker = BlockTracker {
        source: input,
        events: parser.into_offset_iter(),
        depth: 0,
        blocks: 0,
        last_start: 0,
        table_start: None,
    };
    let mut writer = Writer::new(tracker, Some(cwd), width);
    writer.needs_newline = context.needs_newline;
    writer.has_prior_output = context.has_output;
    writer.run();
    let end_context = RenderContext {
        needs_newline: writer.needs_newline,
        has_output: context.has_output || !writer.text.lines.is_empty(),
    };
    StreamingMarkdown {
        lines: writer.text.lines,
        last_block_start: (writer.iter.blocks > 1).then_some(writer.iter.last_start),
        first_table_start: writer.iter.table_start,
        has_references,
        end_context,
    }
}

struct BlockTracker<'a, I> {
    source: &'a str,
    events: I,
    depth: usize,
    blocks: usize,
    last_start: usize,
    table_start: Option<usize>,
}

impl<'a, I: Iterator<Item = (Event<'a>, Range<usize>)>> Iterator for BlockTracker<'a, I> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (event, range) = self.events.next()?;
        if self.depth == 0 && matches!(&event, Event::Start(_) | Event::Rule | Event::Html(_)) {
            self.blocks += 1;
            let line_start = self.source[..range.start]
                .rfind('\n')
                .map_or(0, |newline| newline + 1);
            // Parser offsets may skip indentation that changes block semantics.
            self.last_start = if self.source[line_start..range.start]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t'))
            {
                line_start
            } else {
                range.start
            };
        }
        if matches!(&event, Event::Start(Tag::Table(_))) && self.table_start.is_none() {
            // A nested table keeps its entire enclosing top-level block mutable.
            self.table_start = Some(self.last_start);
        }
        match event {
            Event::Start(_) => self.depth += 1,
            Event::End(_) => self.depth = self.depth.saturating_sub(1),
            _ => {}
        }
        Some(event)
    }
}
