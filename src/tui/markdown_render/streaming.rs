//! Source offsets collected in the canonical writer's parser pass.

use std::{ops::Range, path::Path};

use pulldown_cmark::{BrokenLink, Event, Parser, Tag, TagEnd};
use ratatui::text::Line;

use super::{ListTail, ReferenceContext, Writer, markdown_options};

/// Root formatting state carried only across completed top-level blocks.
#[derive(Clone, Copy, Default)]
pub(crate) struct RenderContext {
    needs_newline: bool,
    has_output: bool,
}

impl RenderContext {
    pub(super) fn after_list_prefix(rows: usize) -> Self {
        Self {
            needs_newline: false,
            has_output: rows > 0,
        }
    }
}

pub(crate) struct StreamingMarkdown {
    pub lines: Vec<Line<'static>>,
    pub last_block_start: Option<usize>,
    pub first_table_start: Option<usize>,
    pub references: ReferenceContext,
    pub end_context: RenderContext,
    /// Spaces omitted after a final root-paragraph raw text span.
    pub text_tail_spaces: Option<usize>,
    pub list_tail: Option<ListTail>,
    pub parsed_bytes: usize,
    #[cfg(test)]
    pub seed_bytes: usize,
}

pub(super) enum RenderOrigin<'a> {
    Root(RenderContext),
    List(&'a ListTail),
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
    render_events(
        input,
        parser.into_offset_iter(),
        width,
        cwd,
        RenderOrigin::Root(context),
        references,
    )
}

pub(super) fn render_events<'a>(
    input: &'a str,
    events: impl Iterator<Item = (Event<'a>, Range<usize>)>,
    width: Option<usize>,
    cwd: &Path,
    origin: RenderOrigin<'_>,
    references: ReferenceContext,
) -> StreamingMarkdown {
    let (context, seed) = match origin {
        RenderOrigin::Root(context) => (context, None),
        RenderOrigin::List(tail) => (
            RenderContext::after_list_prefix(tail.row_start),
            tail.seed.as_ref(),
        ),
    };
    #[cfg(test)]
    let mut seed_bytes = 0;
    let tracker = BlockTracker {
        source: input,
        events,
        depth: 0,
        blocks: 0,
        last_start: 0,
        table_start: None,
        paragraph_range: None,
        text_end: None,
        list_tail: None,
        list_end: 0,
        list_items: 0,
        starts_last_item: false,
    };
    let mut writer = Writer::new(tracker, Some(cwd), width);
    writer.needs_newline = context.needs_newline;
    writer.has_prior_output = context.has_output;
    if let Some(seed) = seed {
        writer.current_line_style = seed.style;
        writer.current_line_content = Some(seed.clone());
        #[cfg(test)]
        {
            seed_bytes += seed
                .spans
                .iter()
                .map(|span| span.content.len())
                .sum::<usize>();
        }
    }
    while let Some(event) = writer.iter.next() {
        writer.prepare_for_event(&event);
        if writer.iter.starts_last_item
            && let Some(tail) = &mut writer.iter.list_tail
        {
            tail.row_start = writer.text.lines.len();
            tail.seed = writer.current_line_content.as_ref().map(|line| {
                let spans = writer
                    .current_initial_indent
                    .iter()
                    .chain(&line.spans)
                    .cloned();
                let seed = Line::from_iter(spans).style(writer.current_line_style);
                #[cfg(test)]
                {
                    seed_bytes += seed
                        .spans
                        .iter()
                        .map(|span| span.content.len())
                        .sum::<usize>();
                }
                seed
            });
        }
        writer.handle_event(event);
    }
    writer.flush_current_line();
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
        list_tail: writer
            .iter
            .list_tail
            .filter(|_| writer.iter.list_end == input.len()),
        parsed_bytes: input.len(),
        #[cfg(test)]
        seed_bytes,
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
    list_tail: Option<ListTail>,
    list_end: usize,
    list_items: usize,
    starts_last_item: bool,
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
        self.starts_last_item = false;
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
            self.list_items = 0;
            self.list_end = range.end;
            self.list_tail = match &event {
                Event::Start(Tag::List(index)) => Some(ListTail {
                    root_start: self.last_start,
                    source_start: self.last_start,
                    row_start: 0,
                    index: *index,
                    loose: false,
                    has_tight_paragraph: false,
                    seed: None,
                }),
                _ => None,
            };
        }
        if let Some(tail) = &mut self.list_tail {
            if self.depth == 1 && matches!(&event, Event::Start(Tag::Item)) {
                tail.source_start = range.start;
                if self.list_items > 0
                    && let Some(index) = &mut tail.index
                {
                    *index += 1;
                }
                self.list_items += 1;
                // Only the final item needs a pending-row snapshot. Capturing
                // every item can repeatedly copy an HTML-extended prior row.
                self.starts_last_item = range.end == self.list_end;
            }
            if self.depth == 2 && matches!(&event, Event::Start(Tag::Paragraph)) {
                tail.loose = true;
            }
            if self.depth == 2
                && matches!(
                    &event,
                    Event::Text(_)
                        | Event::Code(_)
                        | Event::InlineHtml(_)
                        | Event::Start(
                            Tag::Emphasis
                                | Tag::Strong
                                | Tag::Strikethrough
                                | Tag::Link { .. }
                                | Tag::Image { .. }
                        )
                )
            {
                tail.has_tight_paragraph = true;
            }
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
