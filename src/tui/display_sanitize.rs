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

pub(crate) fn sanitize_display_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            strip_escape_sequence(&mut chars);
            continue;
        }

        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                output.push('\n');
            }
            '\n' => output.push('\n'),
            '\t' => output.push_str("    "),
            ch if ch.is_control() => {}
            ch => output.push(ch),
        }
    }
    output
}

pub(crate) fn sanitize_display_line(input: &str) -> String {
    sanitize_display_text(input)
        .replace('\n', "")
        .trim_end()
        .to_string()
}

pub(crate) fn sanitize_display_line_segments(line: &Line<'_>) -> Line<'static> {
    let spans = line
        .spans
        .iter()
        .map(|span| Span {
            content: sanitize_display_line(span.content.as_ref()).into(),
            style: span.style,
        })
        .collect::<Vec<_>>();
    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

fn strip_escape_sequence<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
        Some(']') => {
            chars.next();
            strip_string_control_sequence(chars);
        }
        Some('P' | 'X' | '^' | '_') => {
            chars.next();
            strip_string_control_sequence(chars);
        }
        Some(_) => {
            chars.next();
        }
        None => {}
    }
}

fn strip_string_control_sequence<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    let mut previous_escape = false;
    for next in chars.by_ref() {
        if next == '\u{7}' || (previous_escape && next == '\\') {
            break;
        }
        previous_escape = next == '\u{1b}';
    }
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
        assert_eq!(sanitized.spans[0].style.fg, Some(Color::Red));
        assert!(!sanitized.to_string().contains('\r'));
        assert!(!sanitized.to_string().contains('\u{1b}'));
    }
}
