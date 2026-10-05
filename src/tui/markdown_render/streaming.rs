//! Source offsets collected in the canonical writer's parser pass.

use std::{ops::Range, path::Path};

use pulldown_cmark::{BrokenLink, Event, Parser, Tag, TagEnd};
use ratatui::text::Line;

use super::{ReferenceContext, Writer, markdown_options};

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
    pub references: ReferenceContext,
    pub end_context: RenderContext,
    /// Spaces omitted after a final root-paragraph raw text span.
    pub text_tail_spaces: Option<usize>,
}

pub(crate) fn render_streaming_markdown(
    input: &str,
    width: Option<usize>,
    cwd: &Path,
    context: RenderContext,
    references: &ReferenceContext,
) -> StreamingMarkdown {
    let parser = Parser::new_with_broken_link_callback(
        input,
        markdown_options(),
        Some(|link: BrokenLink<'_>| references.resolve(link.reference.as_ref())),
    );
    let references = ReferenceContext::from_definitions(parser.reference_definitions());
    let tracker = BlockTracker {
        source: input,
        events: parser.into_offset_iter(),
        depth: 0,
        blocks: 0,
        last_start: 0,
        table_start: None,
        paragraph_range: None,
        text_end: None,
    };
    let mut writer = Writer::new(tracker, Some(cwd), width);
    writer.needs_newline = context.needs_newline;
    writer.has_prior_output = context.has_output;
    writer.run();
    let end_context = RenderContext {
        needs_newline: writer.needs_newline,
        has_output: context.has_output || !writer.text.lines.is_empty(),
    };
    let text_tail_spaces = writer.iter.text_tail_spaces();
    StreamingMarkdown {
        lines: writer.text.lines,
        last_block_start: (writer.iter.blocks > 1).then_some(writer.iter.last_start),
        first_table_start: writer.iter.table_start,
        references,
        end_context,
        text_tail_spaces,
    }
}

struct BlockTracker<'a, I> {
    source: &'a str,
    events: I,
    depth: usize,
    blocks: usize,
    last_start: usize,
    table_start: Option<usize>,
    paragraph_range: Option<Range<usize>>,
    text_end: Option<usize>,
}

impl<I> BlockTracker<'_, I> {
    fn text_tail_spaces(&self) -> Option<usize> {
        let end = self.text_end?;
        let paragraph = self.paragraph_range.as_ref()?;
        if paragraph.end != self.source.len() {
            return None;
        }
        let line_start = self.source[..end]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        if line_start > paragraph.start {
            let first = self.source[line_start..].chars().next()?;
            // Empty list items cannot interrupt a paragraph yet. Ordinary text
            // after a prefix such as "1. " can still change the block boundary.
            if !(first.is_ascii_alphabetic()
                || (!first.is_ascii()
                    && !first.is_whitespace()
                    && !first.is_control()
                    && first != '\u{feff}'))
            {
                return None;
            }
        }
        let spaces = &self.source[end..];
        (!spaces.is_empty() && spaces.bytes().all(|byte| byte == b' ')).then_some(spaces.len())
    }
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
            // A leading definition candidate can disappear after an ordinary
            // destination is appended. Nested paragraphs have other owners.
            self.paragraph_range = (matches!(&event, Event::Start(Tag::Paragraph))
                && !self.source[range.start..].starts_with('['))
            .then_some(range.clone());
            self.text_end = None;
        }
        if !matches!(&event, Event::End(TagEnd::Paragraph)) {
            self.text_end = match &event {
                Event::Text(text)
                    if self.depth == 1
                        && self.source[range.clone()] == **text
                        && text.chars().next_back().is_some_and(|ch| {
                            ch.is_alphanumeric()
                                || (!ch.is_ascii()
                                    && !ch.is_whitespace()
                                    && !ch.is_control()
                                    && ch != '\u{feff}')
                                || matches!(ch, '.' | ',' | ':' | ';' | '!' | '?')
                        }) =>
                {
                    Some(range.end)
                }
                _ => None,
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
