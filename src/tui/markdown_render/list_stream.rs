//! Parser context for a suffix that begins at a retained root-list item.

use std::path::Path;

use pulldown_cmark::{BrokenLink, Event, Parser, Tag};
use ratatui::text::Line;

use super::streaming::{RenderOrigin, StreamingMarkdown, render_events};
use super::{ReferenceContext, markdown_options};

pub(crate) struct ListTail {
    pub root_start: usize,
    pub source_start: usize,
    pub row_start: usize,
    pub index: Option<u64>,
    /// Paragraph events observed in the full rendered list.
    pub loose: bool,
    /// Tight paragraphs omit their paragraph events in pulldown-cmark.
    pub has_tight_paragraph: bool,
    /// The writer may append an HTML-first item to the preceding pending row.
    pub seed: Option<Line<'static>>,
}

impl ListTail {
    pub fn shift(&mut self, source_start: usize, row_start: usize) {
        self.root_start += source_start;
        self.source_start += source_start;
        self.row_start += row_start;
    }

    /// A block-only list can be loose without emitting paragraph events.
    /// A visible first item exposes the parser's list-wide spacing decision.
    pub fn detect_block_only_tightness(&mut self, source: &str) -> usize {
        if self.loose || self.has_tight_paragraph {
            return 0;
        }
        let source = &source[self.root_start..];
        let Some(marker) = marker_prefix(source) else {
            return 0;
        };
        let probe = format!("{marker} stream-boundary\n{source}");
        let mut depth = 0;
        for event in Parser::new_ext(&probe, markdown_options()) {
            match event {
                Event::Start(tag) => {
                    if depth == 2 && matches!(tag, Tag::Paragraph) {
                        self.loose = true;
                    }
                    depth += 1;
                }
                Event::End(_) => depth -= 1,
                _ => {}
            }
        }
        probe.len()
    }
}

pub(crate) fn render_list_continuation(
    input: &str,
    width: Option<usize>,
    cwd: &Path,
    references: &ReferenceContext,
    tail: &ListTail,
) -> Option<StreamingMarkdown> {
    let marker = marker_prefix(input)?;
    let separator = if tail.loose { "\n\n" } else { "\n" };
    let prefix = format!("{marker} stream-boundary{separator}");
    let prefix_len = prefix.len();
    let source = format!("{prefix}{input}");
    let parser = Parser::new_with_broken_link_callback(
        &source,
        markdown_options(),
        Some(|link: BrokenLink<'_>| references.resolve(link.reference.as_ref())),
    );
    let definitions = ReferenceContext::from_definitions(parser.reference_definitions());
    let mut depth = 0;
    let mut loose = false;
    let mut spills_prefix = false;
    let events = parser.into_offset_iter().filter_map(|(mut event, range)| {
        let root = depth == 0;
        if depth == 1
            && matches!(&event, Event::Start(Tag::Item))
            && range.start < prefix_len
            && range.end > prefix_len
        {
            // The candidate became a lazy continuation of the synthetic item.
            spills_prefix = true;
        }
        if depth == 2 && matches!(&event, Event::Start(Tag::Paragraph)) {
            loose = true;
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        if range.end <= prefix_len {
            return None;
        }
        if root
            && let Event::Start(Tag::List(index)) = &mut event
            && index.is_some() == tail.index.is_some()
        {
            *index = tail.index;
        }
        Some((
            event,
            range.start.saturating_sub(prefix_len)..range.end - prefix_len,
        ))
    });
    let mut rendered = render_events(
        input,
        events,
        width,
        cwd,
        RenderOrigin::List(tail),
        definitions,
    );
    if let Some(tail) = &mut rendered.list_tail {
        tail.loose |= loose;
    }
    if spills_prefix {
        rendered.list_tail = None;
    }
    rendered.parsed_bytes = source.len();
    Some(rendered)
}

fn marker_prefix(source: &str) -> Option<&str> {
    let body = source.trim_start_matches(' ');
    let indent = source.len() - body.len();
    let first = *body.as_bytes().first()?;
    let marker_len = if matches!(first, b'-' | b'*' | b'+') {
        1
    } else {
        let digits = body.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || !matches!(body.as_bytes().get(digits)?, b'.' | b')') {
            return None;
        }
        digits + 1
    };
    let end = indent + marker_len;
    if source
        .as_bytes()
        .get(end)
        .is_some_and(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
    {
        return None;
    }
    source.get(..end)
}
