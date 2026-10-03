//! Display-text sanitization boundary for the TUI.
//!
//! Any user/model/tool text that can reach a terminal `Print` command or a
//! markdown display collector must pass through this module first. The rule is
//! intentionally centralized: renderers should not open-code their own
//! ANSI/control-character handling, because missed call sites can move the
//! terminal cursor and corrupt the visible transcript.
//!
//! The sanitizer preserves visible text and line boundaries, but removes
//! terminal side effects. Raw payloads may still be persisted elsewhere when
//! needed; this module defines the display contract only.

use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Default)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    Intermediate,
    Csi,
    StringControl,
    StringEscape,
}

#[derive(Clone, Copy, Debug, Default)]
enum Tabs {
    #[default]
    Expand,
    Preserve,
}

/// Removes terminal side effects with constant-size state across text deltas.
#[derive(Clone, Debug, Default)]
pub(crate) struct StreamSanitizer {
    escape: EscapeState,
    after_cr: bool,
    tabs: Tabs,
}

impl StreamSanitizer {
    pub(crate) fn push_delta(&mut self, input: &str) -> String {
        let mut output = String::with_capacity(input.len());
        self.write(input, |ch| output.push(ch));
        output
    }

    /// A bounded consumer can evict output while parsing instead of allocating
    /// a complete sanitized copy of an arbitrarily large incoming chunk.
    pub(crate) fn write(&mut self, input: &str, mut emit: impl FnMut(char)) {
        for ch in input.chars() {
            let after_cr = std::mem::take(&mut self.after_cr);
            // A transcript line is a recovery boundary, even inside an
            // unfinished escape. Do not let malformed metadata hide later lines.
            if matches!(ch, '\r' | '\n') {
                self.escape = EscapeState::Ground;
                if ch == '\r' || !after_cr {
                    emit('\n');
                }
                self.after_cr = ch == '\r';
                continue;
            }
            if matches!(ch, '\u{18}' | '\u{1a}') {
                self.escape = EscapeState::Ground;
                continue;
            }
            match self.escape {
                EscapeState::Ground => match ch {
                    '\u{1b}' => self.escape = EscapeState::Escape,
                    '\u{9b}' => self.escape = EscapeState::Csi,
                    '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => {
                        self.escape = EscapeState::StringControl;
                    }
                    '\t' => match self.tabs {
                        Tabs::Expand => {
                            for _ in 0..4 {
                                emit(' ');
                            }
                        }
                        Tabs::Preserve => emit('\t'),
                    },
                    ch if ch.is_control() => {}
                    ch => emit(ch),
                },
                EscapeState::Escape => {
                    self.escape = match ch {
                        '[' => EscapeState::Csi,
                        ']' | 'P' | 'X' | '^' | '_' => EscapeState::StringControl,
                        ' '..='/' => EscapeState::Intermediate,
                        '\u{1b}' => EscapeState::Escape,
                        // Inline controls such as NUL do not terminate an
                        // escape; logical newlines recover before this match.
                        ch if ch.is_control() => EscapeState::Escape,
                        _ => EscapeState::Ground,
                    };
                }
                EscapeState::Intermediate | EscapeState::Csi => {
                    if ch == '\u{1b}' {
                        self.escape = EscapeState::Escape;
                    } else if ('@'..='~').contains(&ch)
                        || (matches!(self.escape, EscapeState::Intermediate)
                            && ('0'..='?').contains(&ch))
                    {
                        self.escape = EscapeState::Ground;
                    }
                }
                EscapeState::StringControl | EscapeState::StringEscape => {
                    self.escape = match ch {
                        '\u{7}' | '\u{9c}' => EscapeState::Ground,
                        '\\' if matches!(self.escape, EscapeState::StringEscape) => {
                            EscapeState::Ground
                        }
                        '\u{1b}' => EscapeState::StringEscape,
                        _ => EscapeState::StringControl,
                    };
                }
            }
        }
    }
}

pub(crate) fn sanitize_display_text(input: &str) -> String {
    StreamSanitizer::default().push_delta(input)
}

pub(crate) fn sanitize_paste_text(input: &str) -> String {
    StreamSanitizer {
        tabs: Tabs::Preserve,
        ..StreamSanitizer::default()
    }
    .push_delta(input)
}

pub(crate) fn sanitize_display_line(input: &str) -> String {
    sanitize_display_text(input)
        .replace('\n', "")
        .trim_end()
        .to_string()
}

pub(crate) fn sanitize_display_line_segments(line: &Line<'_>) -> Line<'static> {
    let mut sanitizer = StreamSanitizer::default();
    let sanitized_spans = line
        .spans
        .iter()
        .map(|span| Span {
            // A styled Line is already one physical row. Source ingestion,
            // not Ratatui's Line formatter, owns explicit line boundaries.
            content: sanitizer
                .push_delta(span.content.as_ref())
                .replace('\n', "")
                .into(),
            style: span.style,
        })
        .collect::<Vec<_>>();
    let sanitized_len = sanitized_spans
        .iter()
        .map(|span| span.content.len())
        .sum::<usize>();
    let mut spans = visible_grapheme_spans(sanitized_spans);
    if spans.iter().map(|span| span.content.len()).sum::<usize>() < sanitized_len {
        // Removing an invisible separator can join neighboring visible
        // clusters. Re-segment once so style spans and plain-text widths agree.
        spans = visible_grapheme_spans(spans);
    }
    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

fn visible_grapheme_spans(source: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let text = source
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let mut span_ends = source
        .iter()
        .scan(0, |end, span| {
            *end += span.content.len();
            Some(*end)
        })
        .peekable();
    let needs_projection = text.grapheme_indices(true).any(|(offset, grapheme)| {
        while span_ends.next_if(|end| *end <= offset).is_some() {}
        super::text_wrap::grapheme_width(grapheme) == 0
            || span_ends
                .peek()
                .is_some_and(|end| *end < offset + grapheme.len())
    });
    drop(span_ends);
    if !needs_projection {
        // Keep already valid style boundaries stable for row-cache equality.
        return source;
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut span_index = 0;
    let mut span_end = source.first().map_or(0, |span| span.content.len());
    for (offset, grapheme) in text.grapheme_indices(true) {
        while offset >= span_end && span_index + 1 < source.len() {
            span_index += 1;
            span_end += source[span_index].content.len();
        }
        if super::text_wrap::grapheme_width(grapheme) == 0 {
            continue;
        }
        // Ratatui segments individual spans. Joining across style boundaries
        // here preserves the base character's complete visible cluster.
        let style = source[span_index].style;
        if let Some(last) = spans.last_mut()
            && last.style == style
        {
            last.content.to_mut().push_str(grapheme);
        } else {
            spans.push(Span::styled(grapheme.to_owned(), style));
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};

    use super::{sanitize_display_line, sanitize_display_line_segments, sanitize_display_text};

    #[test]
    fn removes_terminal_controls_from_display_text() {
        assert_eq!(
            sanitize_display_text(
                "start\u{1b}[31mred\u{1b}[0m\rnext\u{8}!\u{1b}]0;title\u{7}\tend"
            ),
            "startred\nnext!    end"
        );
    }

    #[test]
    fn review_regression_unterminated_controls_recover_at_line_boundaries() {
        for prefix in [
            "\u{1b}]payload",
            "\u{1b}Ppayload",
            "\u{1b}Xpayload",
            "\u{1b}^payload",
            "\u{1b}_payload",
            "\u{90}payload",
            "\u{98}payload",
            "\u{9d}payload",
            "\u{9e}payload",
            "\u{9f}payload",
            "\u{1b}]payload\u{1b}",
            "\u{1b}[31",
            "\u{1b}(",
        ] {
            for newline in ["\n", "\r", "\r\n"] {
                let source = format!("before{prefix}{newline}after\u{1b}[31mred\u{1b}[0m");
                assert_eq!(
                    sanitize_display_text(&source),
                    "before\nafterred",
                    "{source:?}"
                );
                let boundaries = source
                    .char_indices()
                    .map(|(index, _)| index)
                    .chain([source.len()])
                    .collect::<Vec<_>>();
                for &first in &boundaries {
                    for &second in boundaries.iter().filter(|&&index| index >= first) {
                        let mut sanitizer = super::StreamSanitizer::default();
                        let output = [&source[..first], &source[first..second], &source[second..]]
                            .into_iter()
                            .map(|chunk| sanitizer.push_delta(chunk))
                            .collect::<String>();
                        assert_eq!(output, "before\nafterred", "{source:?} at {first}/{second}");
                    }
                }
            }
        }
    }

    #[test]
    fn review_regression_escape_newline_preserves_next_printable_character() {
        for newline in ["\n", "\r", "\r\n"] {
            let mut sanitizer = super::StreamSanitizer::default();
            assert_eq!(sanitizer.push_delta("\u{1b}"), "");
            assert_eq!(sanitizer.push_delta(newline), "\n");
            assert_eq!(sanitizer.push_delta("abc"), "abc");
        }
    }

    #[test]
    fn display_line_never_contains_cursor_moving_controls() {
        assert_eq!(
            sanitize_display_line("abc\r\u{1b}[2Kdef\u{7}\tghi\u{1b}Ppayload\u{1b}\\j"),
            "abcdef    ghij"
        );
    }

    #[test]
    fn line_segments_are_sanitized_without_losing_style() {
        let line = Line::from(vec![Span::styled(
            "ok\r\u{1b}[31mred\u{1b}[0m",
            Style::default().fg(Color::Red),
        )]);

        let sanitized = sanitize_display_line_segments(&line);

        assert_eq!(sanitized.to_string(), "okred");
        assert_eq!(sanitized.spans[0].content, "okred");
        assert_eq!(sanitized.spans[0].style.fg, Some(Color::Red));
        assert!(!sanitized.to_string().contains('\r'));
        assert!(!sanitized.to_string().contains('\u{1b}'));
    }

    #[test]
    fn every_character_boundary_preserves_escape_and_crlf_state() {
        let cases = [
            ("a\u{1b}[31mred\u{1b}[0mz", "aredz"),
            (
                "a\u{1b}]8;;https://example.test\u{1b}\\link\u{1b}]8;;\u{7}z",
                "alinkz",
            ),
            ("a\u{1b}Ppayload\u{1b}\\z", "az"),
            ("a\u{1b}Xpayload\u{1b}\\z", "az"),
            ("a\u{1b}^payload\u{1b}\\z", "az"),
            ("a\u{1b}_payload\u{1b}\\z", "az"),
            ("a\u{1b}(Bz", "az"),
            ("a\u{1b}\u{0}[31mred\u{1b}[0mz", "aredz"),
            ("a\u{9b}31mred\u{9b}0mz", "aredz"),
            ("a\u{9d}payload\u{9c}z", "az"),
            ("a\r\nb\r\n\r\nc", "a\nb\n\nc"),
            ("a\r\rb\nc\t\u{754c}", "a\n\nb\nc    \u{754c}"),
            ("a\u{1b}[31\u{18}z", "az"),
        ];
        for (source, expected) in cases {
            assert_eq!(sanitize_display_text(source), expected);
            let boundaries = source
                .char_indices()
                .map(|(index, _)| index)
                .chain([source.len()])
                .collect::<Vec<_>>();
            for &first in &boundaries {
                for &second in boundaries.iter().filter(|&&index| index >= first) {
                    let mut sanitizer = super::StreamSanitizer::default();
                    let result = [&source[..first], &source[first..second], &source[second..]]
                        .into_iter()
                        .map(|chunk| sanitizer.push_delta(chunk))
                        .collect::<String>();
                    assert_eq!(result, expected, "{source:?} split at {first}/{second}");
                }
            }
        }
    }

    #[test]
    fn unterminated_controls_retain_no_payload_bytes() {
        let mut sanitizer = super::StreamSanitizer::default();
        assert_eq!(sanitizer.push_delta("before\u{1b}]"), "before");
        let mut emitted = 0;
        sanitizer.write(&"x".repeat(10 * 1024 * 1024), |_| emitted += 1);
        assert_eq!(emitted, 0);
        assert!(std::mem::size_of_val(&sanitizer) < 32);
        assert_eq!(sanitizer.push_delta("\u{1b}"), "");
        assert_eq!(sanitizer.push_delta("\\after"), "after");
        assert_eq!(sanitize_display_text("before\u{1b}[31"), "before");
        assert_eq!(sanitize_display_text("before\u{1b}]payload"), "before");
    }

    #[test]
    fn styled_spans_share_parser_state_and_keep_visible_styles() {
        let line = Line::from(vec![
            Span::raw("left\u{1b}[3"),
            Span::styled("1mred\u{1b}[0", Style::default().fg(Color::Red)),
            Span::raw("mright"),
        ]);
        let sanitized = sanitize_display_line_segments(&line);
        assert_eq!(sanitized.to_string(), "leftredright");
        assert_eq!(sanitized.spans[1].content, "red");
        assert_eq!(sanitized.spans[1].style.fg, Some(Color::Red));
    }
}
