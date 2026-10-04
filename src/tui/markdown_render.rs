mod local_links;
mod references;
mod streaming;
mod table;

use std::path::{Path, PathBuf};

use pulldown_cmark::{
    Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use ratatui::{
    style::Style,
    text::{Line, Span, Text},
};
pub(crate) use references::{ReferenceBudget, ReferenceContext};
pub(crate) use streaming::{RenderContext, render_streaming_markdown};
use table::{TableRenderState, render_table_lines};

use self::local_links::{
    is_local_path_like_link, render_local_link_target, should_render_link_destination,
};
use crate::tui::highlight::highlight_code_to_lines;
use crate::tui::theme::{ThemeToken, theme_color};

#[derive(Default)]
struct MarkdownStyles {
    h1: Style,
    h2: Style,
    h3: Style,
    h4: Style,
    h5: Style,
    h6: Style,
    code: Style,
    emphasis: Style,
    strong: Style,
    strikethrough: Style,
    ordered_list_marker: Style,
    unordered_list_marker: Style,
    link: Style,
    blockquote: Style,
    code_block_lang_tag: Style,
    task_list_marker: Style,
    footnote: Style,
}

impl MarkdownStyles {
    fn new() -> Self {
        Self {
            h1: Style::new()
                .fg(theme_color(ThemeToken::MarkdownHeading))
                .bold()
                .underlined(),
            h2: Style::new()
                .fg(theme_color(ThemeToken::MarkdownHeading))
                .bold(),
            h3: Style::new()
                .fg(theme_color(ThemeToken::MarkdownHeading))
                .bold()
                .italic(),
            h4: Style::new()
                .fg(theme_color(ThemeToken::MarkdownHeading))
                .italic(),
            h5: Style::new()
                .fg(theme_color(ThemeToken::MarkdownHeading))
                .italic(),
            h6: Style::new()
                .fg(theme_color(ThemeToken::MarkdownHeading))
                .italic(),
            code: Style::new().fg(theme_color(ThemeToken::MarkdownCode)),
            emphasis: Style::new()
                .fg(theme_color(ThemeToken::MarkdownItalic))
                .italic(),
            strong: Style::new()
                .fg(theme_color(ThemeToken::MarkdownBold))
                .bold(),
            strikethrough: Style::new().crossed_out(),
            ordered_list_marker: Style::new().fg(theme_color(ThemeToken::MarkdownListBullet)),
            unordered_list_marker: Style::new().fg(theme_color(ThemeToken::MarkdownListBullet)),
            link: Style::new()
                .fg(theme_color(ThemeToken::MarkdownLink))
                .underlined(),
            blockquote: Style::new().fg(theme_color(ThemeToken::MarkdownBlockQuote)),
            code_block_lang_tag: Style::new().fg(theme_color(ThemeToken::TextSecondary)),
            task_list_marker: Style::new().bold(),
            footnote: Style::new().fg(theme_color(ThemeToken::TextMuted)),
        }
    }
}

#[derive(Clone, Debug)]
struct IndentContext {
    prefix: Vec<Span<'static>>,
    marker: Option<Vec<Span<'static>>>,
    is_list: bool,
}

impl IndentContext {
    fn new(prefix: Vec<Span<'static>>, marker: Option<Vec<Span<'static>>>, is_list: bool) -> Self {
        Self {
            prefix,
            marker,
            is_list,
        }
    }
}

#[derive(Clone, Debug)]
struct LinkState {
    destination: String,
    show_destination: bool,
    local_target_display: Option<String>,
}

#[cfg(test)]
pub(crate) fn render_markdown_text(input: &str) -> Text<'static> {
    render_markdown_text_with_width(input, None)
}

pub(crate) fn render_markdown_text_with_width(input: &str, width: Option<usize>) -> Text<'static> {
    let cwd = std::env::current_dir().ok();
    render_markdown_text_with_width_and_cwd(input, width, cwd.as_deref())
}

pub(crate) fn render_markdown_text_with_width_and_cwd(
    input: &str,
    width: Option<usize>,
    cwd: Option<&Path>,
) -> Text<'static> {
    let parser = Parser::new_ext(input, markdown_options());
    let mut writer = Writer::new(parser, cwd, width);
    writer.run();
    writer.text
}

fn markdown_options() -> Options {
    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES
}

struct Writer<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    iter: I,
    text: Text<'static>,
    styles: MarkdownStyles,
    inline_styles: Vec<Style>,
    indent_stack: Vec<IndentContext>,
    list_indices: Vec<Option<u64>>,
    link: Option<LinkState>,
    needs_newline: bool,
    has_prior_output: bool,
    pending_marker_line: bool,
    in_paragraph: bool,
    in_code_block: bool,
    code_block_lang: Option<String>,
    code_block_buffer: String,
    cwd: Option<PathBuf>,
    line_ends_with_local_link_target: bool,
    pending_local_link_soft_break: bool,
    current_line_content: Option<Line<'static>>,
    current_initial_indent: Vec<Span<'static>>,
    current_subsequent_indent: Vec<Span<'static>>,
    current_line_style: Style,
    current_line_in_code_block: bool,
    table: Option<TableRenderState>,
    width: Option<usize>,
}

impl<'a, I> Writer<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    fn new(iter: I, cwd: Option<&Path>, width: Option<usize>) -> Self {
        Self {
            iter,
            text: Text::default(),
            styles: MarkdownStyles::new(),
            inline_styles: Vec::new(),
            indent_stack: Vec::new(),
            list_indices: Vec::new(),
            link: None,
            needs_newline: false,
            has_prior_output: false,
            pending_marker_line: false,
            in_paragraph: false,
            in_code_block: false,
            code_block_lang: None,
            code_block_buffer: String::new(),
            cwd: cwd.map(Path::to_path_buf),
            line_ends_with_local_link_target: false,
            pending_local_link_soft_break: false,
            current_line_content: None,
            current_initial_indent: Vec::new(),
            current_subsequent_indent: Vec::new(),
            current_line_style: Style::default(),
            current_line_in_code_block: false,
            table: None,
            width,
        }
    }

    fn run(&mut self) {
        while let Some(event) = self.iter.next() {
            self.prepare_for_event(&event);
            self.handle_event(event);
        }
        self.flush_current_line();
    }

    fn prepare_for_event(&mut self, event: &Event<'a>) {
        if !self.pending_local_link_soft_break {
            return;
        }

        if matches!(event, Event::Text(text) if text.trim_start().starts_with(':')) {
            self.pending_local_link_soft_break = false;
            return;
        }

        self.pending_local_link_soft_break = false;
        self.push_line(Line::default());
    }

    fn handle_event(&mut self, event: Event<'a>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.code(code),
            Event::SoftBreak => self.soft_break(),
            Event::HardBreak => self.hard_break(),
            Event::Rule => {
                self.flush_current_line();
                if self.has_prior_output || !self.text.lines.is_empty() {
                    self.push_blank_line();
                }
                self.push_line(Line::from("———"));
                self.needs_newline = true;
            }
            Event::Html(html) => self.html(html, false),
            Event::InlineHtml(html) => self.html(html, true),
            Event::InlineMath(math) | Event::DisplayMath(math) => self.code(math),
            Event::TaskListMarker(checked) => self.task_list_marker(checked),
            Event::FootnoteReference(name) => self.footnote_reference(name),
        }
    }

    fn start_tag(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote(_) => self.start_blockquote(),
            Tag::CodeBlock(kind) => {
                let indent = match kind {
                    CodeBlockKind::Fenced(_) => None,
                    CodeBlockKind::Indented => Some(Span::from(" ".repeat(4))),
                };
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => Some(lang.to_string()),
                    CodeBlockKind::Indented => None,
                };
                self.start_codeblock(lang, indent);
            }
            Tag::List(start) => self.start_list(start),
            Tag::Item => self.start_item(),
            Tag::Table(alignments) => self.start_table(alignments),
            Tag::TableHead => self.start_table_head(),
            Tag::TableRow => self.start_table_row(),
            Tag::TableCell => self.start_table_cell(),
            Tag::Emphasis => self.push_inline_style(self.styles.emphasis),
            Tag::Strong => self.push_inline_style(self.styles.strong),
            Tag::Strikethrough => self.push_inline_style(self.styles.strikethrough),
            Tag::Link { dest_url, .. } => self.push_link(dest_url.to_string()),
            Tag::HtmlBlock
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript
            | Tag::FootnoteDefinition(_)
            | Tag::Image { .. }
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.end_paragraph(),
            TagEnd::Heading(_) => self.end_heading(),
            TagEnd::BlockQuote(_) => self.end_blockquote(),
            TagEnd::CodeBlock => self.end_codeblock(),
            TagEnd::List(_) => self.end_list(),
            TagEnd::Item => {
                self.indent_stack.pop();
                self.pending_marker_line = false;
            }
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => self.end_table_head(),
            TagEnd::TableRow => self.end_table_row(),
            TagEnd::TableCell => self.end_table_cell(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_inline_style(),
            TagEnd::Link => self.pop_link(),
            TagEnd::HtmlBlock
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::FootnoteDefinition
            | TagEnd::Image
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn start_paragraph(&mut self) {
        if self.needs_newline {
            self.push_blank_line();
        }
        self.push_line(Line::default());
        self.needs_newline = false;
        self.in_paragraph = true;
    }

    fn end_paragraph(&mut self) {
        self.needs_newline = true;
        self.in_paragraph = false;
        self.pending_marker_line = false;
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        if self.needs_newline {
            self.push_line(Line::default());
            self.needs_newline = false;
        }
        let heading_style = match level {
            HeadingLevel::H1 => self.styles.h1,
            HeadingLevel::H2 => self.styles.h2,
            HeadingLevel::H3 => self.styles.h3,
            HeadingLevel::H4 => self.styles.h4,
            HeadingLevel::H5 => self.styles.h5,
            HeadingLevel::H6 => self.styles.h6,
        };
        let content = format!("{} ", "#".repeat(level as usize));
        self.push_line(Line::from(vec![Span::styled(content, heading_style)]));
        self.push_inline_style(heading_style);
        self.needs_newline = false;
    }

    fn end_heading(&mut self) {
        self.needs_newline = true;
        self.pop_inline_style();
    }

    fn start_blockquote(&mut self) {
        if self.needs_newline {
            self.push_blank_line();
            self.needs_newline = false;
        }
        self.indent_stack
            .push(IndentContext::new(vec![Span::from("> ")], None, false));
    }

    fn end_blockquote(&mut self) {
        self.indent_stack.pop();
        self.needs_newline = true;
    }

    fn text(&mut self, text: CowStr<'a>) {
        if self.table.is_some() {
            self.push_table_text(&text);
            return;
        }
        if self.suppressing_local_link_label() {
            return;
        }
        self.line_ends_with_local_link_target = false;
        if self.pending_marker_line {
            self.push_line(Line::default());
        }
        self.pending_marker_line = false;

        if self.in_code_block && self.code_block_lang.is_some() {
            self.code_block_buffer.push_str(&text);
            return;
        }

        if self.in_code_block && !self.needs_newline {
            let has_content = self
                .current_line_content
                .as_ref()
                .map(|line| !line.spans.is_empty())
                .unwrap_or(false);
            if has_content {
                self.push_line(Line::default());
            }
        }

        for (idx, line) in text.lines().enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if idx > 0 {
                self.push_line(Line::default());
            }
            let span = Span::styled(
                line.to_string(),
                self.inline_styles.last().copied().unwrap_or_default(),
            );
            self.push_span(span);
        }
        self.needs_newline = false;
    }

    fn code(&mut self, code: CowStr<'a>) {
        if self.table.is_some() {
            self.push_table_text(&code);
            return;
        }
        if self.suppressing_local_link_label() {
            return;
        }
        self.line_ends_with_local_link_target = false;
        if self.pending_marker_line {
            self.push_line(Line::default());
            self.pending_marker_line = false;
        }
        let span = Span::styled(code.into_string(), self.styles.code);
        self.push_span(span);
    }

    fn html(&mut self, html: CowStr<'a>, inline: bool) {
        if self.table.is_some() {
            self.push_table_text(&html);
            return;
        }
        if self.suppressing_local_link_label() {
            return;
        }
        self.line_ends_with_local_link_target = false;
        self.pending_marker_line = false;
        for (idx, line) in html.lines().enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if idx > 0 {
                self.push_line(Line::default());
            }
            let style = self.inline_styles.last().copied().unwrap_or_default();
            self.push_span(Span::styled(line.to_string(), style));
        }
        self.needs_newline = !inline;
    }

    fn hard_break(&mut self) {
        if self.table.is_some() {
            self.push_table_text(" ");
            return;
        }
        if self.suppressing_local_link_label() {
            return;
        }
        self.line_ends_with_local_link_target = false;
        self.push_line(Line::default());
    }

    fn soft_break(&mut self) {
        if self.table.is_some() {
            self.push_table_text(" ");
            return;
        }
        if self.suppressing_local_link_label() {
            return;
        }
        if self.line_ends_with_local_link_target {
            self.pending_local_link_soft_break = true;
            self.line_ends_with_local_link_target = false;
            return;
        }
        self.line_ends_with_local_link_target = false;
        self.push_line(Line::default());
    }

    fn start_list(&mut self, index: Option<u64>) {
        if self.list_indices.is_empty() && self.needs_newline {
            self.push_line(Line::default());
        }
        self.list_indices.push(index);
    }

    fn end_list(&mut self) {
        self.list_indices.pop();
        self.needs_newline = true;
    }

    fn start_item(&mut self) {
        self.pending_marker_line = true;
        let depth = self.list_indices.len();
        let is_ordered = self
            .list_indices
            .last()
            .map(Option::is_some)
            .unwrap_or(false);
        let width = depth * 4 - 3;
        let marker = if let Some(last_index) = self.list_indices.last_mut() {
            match last_index {
                None => Some(vec![Span::styled(
                    " ".repeat(width - 1) + "- ",
                    self.styles.unordered_list_marker,
                )]),
                Some(index) => {
                    *index += 1;
                    Some(vec![Span::styled(
                        format!("{:width$}. ", *index - 1),
                        self.styles.ordered_list_marker,
                    )])
                }
            }
        } else {
            None
        };
        let indent_prefix = if depth == 0 {
            Vec::new()
        } else {
            let indent_len = if is_ordered { width + 2 } else { width + 1 };
            vec![Span::from(" ".repeat(indent_len))]
        };
        self.indent_stack
            .push(IndentContext::new(indent_prefix, marker, true));
        self.needs_newline = false;
    }

    fn start_codeblock(&mut self, lang: Option<String>, indent: Option<Span<'static>>) {
        self.flush_current_line();
        if self.has_prior_output || !self.text.lines.is_empty() {
            self.push_blank_line();
        }
        self.in_code_block = true;
        self.code_block_lang = lang
            .as_deref()
            .and_then(|value| value.split([',', ' ', '\t']).next())
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        self.code_block_buffer.clear();
        self.indent_stack.push(IndentContext::new(
            vec![indent.unwrap_or_default()],
            None,
            false,
        ));
        self.needs_newline = true;
    }

    fn task_list_marker(&mut self, checked: bool) {
        let symbol = if checked { "☒ " } else { "☐ " };
        if let Some(ctx) = self.indent_stack.last_mut() {
            ctx.marker = Some(vec![Span::styled(
                symbol.to_string(),
                self.styles.task_list_marker,
            )]);
        }
    }

    fn footnote_reference(&mut self, name: CowStr<'_>) {
        let content = format!("[{}]", name);
        self.push_span(Span::styled(content, self.styles.footnote));
    }

    fn end_codeblock(&mut self) {
        if let Some(lang) = self.code_block_lang.take() {
            let code = std::mem::take(&mut self.code_block_buffer);
            if !code.is_empty() {
                // Show the language tag before the code
                self.push_line(Line::default());
                self.push_span(Span::styled(
                    format!("  {}", lang),
                    self.styles.code_block_lang_tag,
                ));
                let highlighted = highlight_code_to_lines(&code, &lang);
                for line in highlighted {
                    self.push_line(Line::default());
                    for span in line.spans {
                        self.push_span(span);
                    }
                }
            }
        }

        self.needs_newline = true;
        self.in_code_block = false;
        self.indent_stack.pop();
    }

    fn start_table(&mut self, alignments: Vec<Alignment>) {
        self.flush_current_line();
        if self.needs_newline && (self.has_prior_output || !self.text.lines.is_empty()) {
            self.push_blank_line();
        }
        self.table = Some(TableRenderState::new(alignments));
        self.needs_newline = false;
    }

    fn end_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        for line in render_table_lines(&table, self.width) {
            self.push_line(Line::from(line));
            self.flush_current_line();
        }
        self.needs_newline = true;
    }

    fn end_table_head(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.end_header();
        }
    }

    fn start_table_head(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.start_header();
        }
    }

    fn start_table_row(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.start_row();
        }
    }

    fn end_table_row(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.end_row();
        }
    }

    fn start_table_cell(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.start_cell();
        }
    }

    fn end_table_cell(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.end_cell();
        }
    }

    fn push_table_text(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut() {
            table.push_text(text);
        }
    }

    fn push_inline_style(&mut self, style: Style) {
        let current = self.inline_styles.last().copied().unwrap_or_default();
        self.inline_styles.push(current.patch(style));
    }

    fn pop_inline_style(&mut self) {
        self.inline_styles.pop();
    }

    fn push_link(&mut self, dest_url: String) {
        let show_destination = should_render_link_destination(&dest_url);
        self.link = Some(LinkState {
            show_destination,
            local_target_display: if is_local_path_like_link(&dest_url) {
                render_local_link_target(&dest_url, self.cwd.as_deref())
            } else {
                None
            },
            destination: dest_url,
        });
    }

    fn pop_link(&mut self) {
        if let Some(link) = self.link.take() {
            if link.show_destination {
                self.push_span(" (".into());
                self.push_span(Span::styled(link.destination, self.styles.link));
                self.push_span(")".into());
            } else if let Some(local_target_display) = link.local_target_display {
                if self.pending_marker_line {
                    self.push_line(Line::default());
                }
                let style = self
                    .inline_styles
                    .last()
                    .copied()
                    .unwrap_or_default()
                    .patch(self.styles.code);
                self.push_span(Span::styled(local_target_display, style));
                self.line_ends_with_local_link_target = true;
            }
        }
    }

    fn suppressing_local_link_label(&self) -> bool {
        self.link
            .as_ref()
            .and_then(|link| link.local_target_display.as_ref())
            .is_some()
    }

    fn flush_current_line(&mut self) {
        if let Some(line) = self.current_line_content.take() {
            let style = self.current_line_style;
            let mut spans = self.current_initial_indent.clone();
            let mut line = line;
            spans.append(&mut line.spans);
            self.text.lines.push(Line::from_iter(spans).style(style));
            self.current_initial_indent.clear();
            self.current_subsequent_indent.clear();
            self.current_line_in_code_block = false;
            self.line_ends_with_local_link_target = false;
        }
    }

    fn push_line(&mut self, line: Line<'static>) {
        self.flush_current_line();
        let blockquote_active = self
            .indent_stack
            .iter()
            .any(|ctx| ctx.prefix.iter().any(|span| span.content.contains('>')));
        self.current_line_style = if blockquote_active {
            self.styles.blockquote
        } else {
            line.style
        };
        let was_pending = self.pending_marker_line;
        self.current_initial_indent = self.prefix_spans(was_pending);
        self.current_subsequent_indent = self.prefix_spans(false);
        self.current_line_content = Some(line);
        self.current_line_in_code_block = self.in_code_block;
        self.pending_marker_line = false;
        self.line_ends_with_local_link_target = false;
    }

    fn push_span(&mut self, span: Span<'static>) {
        if let Some(line) = self.current_line_content.as_mut() {
            line.push_span(span);
        } else {
            self.push_line(Line::from(vec![span]));
        }
    }

    fn push_blank_line(&mut self) {
        self.flush_current_line();
        if self.indent_stack.iter().all(|ctx| ctx.is_list) {
            self.text.lines.push(Line::default());
        } else {
            self.push_line(Line::default());
            self.flush_current_line();
        }
    }

    fn prefix_spans(&self, pending_marker_line: bool) -> Vec<Span<'static>> {
        let mut prefix = Vec::new();
        let last_marker_index = if pending_marker_line {
            self.indent_stack
                .iter()
                .enumerate()
                .rev()
                .find_map(|(idx, ctx)| ctx.marker.as_ref().map(|_| idx))
        } else {
            None
        };
        let last_list_index = self.indent_stack.iter().rposition(|ctx| ctx.is_list);

        for (idx, ctx) in self.indent_stack.iter().enumerate() {
            if pending_marker_line {
                if Some(idx) == last_marker_index {
                    if let Some(marker) = &ctx.marker {
                        prefix.extend(marker.iter().cloned());
                    }
                    continue;
                }
                if ctx.is_list && last_marker_index.is_some_and(|marker_idx| marker_idx > idx) {
                    continue;
                }
            } else if ctx.is_list && Some(idx) != last_list_index {
                continue;
            }
            prefix.extend(ctx.prefix.iter().cloned());
        }

        prefix
    }
}

#[cfg(test)]
#[path = "markdown_render_tests.rs"]
mod tests;
