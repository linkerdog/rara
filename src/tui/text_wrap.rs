//! Shared source ranges for transcript word wrapping and composer grapheme wrapping.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const TAB_WIDTH: usize = 4;

#[derive(Clone, Copy)]
pub(crate) enum WrapMode {
    Word,
    Grapheme,
}

pub(crate) struct WrapOptions {
    pub width: usize,
    pub initial_indent: usize,
    pub subsequent_indent: usize,
    pub mode: WrapMode,
}

pub(crate) fn grapheme_width(grapheme: &str) -> usize {
    if grapheme == "\t" {
        TAB_WIDTH
    } else if grapheme.contains(char::is_control) {
        0
    } else {
        // Match Ratatui's cell-width policy for visible halfwidth sound marks.
        UnicodeWidthStr::width(grapheme)
            + grapheme
                .chars()
                .filter(|ch| matches!(ch, '\u{ff9e}' | '\u{ff9f}'))
                .count()
    }
}

pub(crate) fn expand_tabs(text: &str) -> String {
    text.replace('\t', &" ".repeat(TAB_WIDTH))
}

pub(crate) fn display_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// Returns a source prefix ending at a whole-grapheme display-column boundary.
pub(crate) fn truncate_to_width(text: &str, width: usize) -> &str {
    if width == 0 {
        return "";
    }
    let mut remaining = width;
    let mut end = 0;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let Some(next) = remaining.checked_sub(grapheme_width(grapheme)) else {
            break;
        };
        remaining = next;
        end = offset + grapheme.len();
    }
    &text[..end]
}

/// Returns a source suffix starting at a whole-grapheme display-column boundary.
pub(crate) fn suffix_to_width(text: &str, width: usize) -> &str {
    if width == 0 {
        return "";
    }
    let mut remaining = width;
    let mut start = text.len();
    for (offset, grapheme) in text.grapheme_indices(true).rev() {
        let Some(next) = remaining.checked_sub(grapheme_width(grapheme)) else {
            break;
        };
        remaining = next;
        start = offset;
    }
    &text[start..]
}

fn is_break_space(grapheme: &str) -> bool {
    grapheme.chars().all(char::is_whitespace) && !grapheme.contains(['\u{a0}', '\u{202f}'])
}

/// Produces only grapheme-boundary ranges, including empty explicit lines.
pub(crate) fn wrap_ranges(input: &str, options: WrapOptions) -> Vec<Range<usize>> {
    wrap_ranges_with_atoms(input, options, &[])
}

/// Atomic spans are sorted, disjoint byte ranges at grapheme boundaries.
pub(crate) fn display_units<'a>(input: &'a str, atoms: &[Range<usize>]) -> Vec<(usize, &'a str)> {
    let mut atom = 0;
    input
        .grapheme_indices(true)
        .filter_map(|(offset, grapheme)| {
            while atoms.get(atom).is_some_and(|range| range.end <= offset) {
                atom += 1;
            }
            match atoms.get(atom) {
                Some(range) if range.start == offset => {
                    input.get(range.clone()).map(|text| (offset, text))
                }
                Some(range) if range.contains(&offset) => None,
                Some(_) | None => Some((offset, grapheme)),
            }
        })
        .collect()
}

pub(crate) fn wrap_ranges_with_atoms(
    input: &str,
    options: WrapOptions,
    atoms: &[Range<usize>],
) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut line_start = 0;
    for line in input.split('\n') {
        let local_atoms = atoms
            .iter()
            .filter(|range| range.start >= line_start && range.end <= line_start + line.len())
            .map(|range| range.start - line_start..range.end - line_start)
            .collect::<Vec<_>>();
        let graphemes = display_units(line, &local_atoms);
        let mut start = 0;
        if graphemes.is_empty() {
            rows.push(line_start..line_start);
        }
        while start < graphemes.len() {
            let indent = if rows.is_empty() {
                options.initial_indent
            } else {
                options.subsequent_indent
            };
            let available = options.width.max(1).saturating_sub(indent);
            let mut end = start;
            let mut columns = 0usize;
            let mut word_break = None;
            while let Some((_, grapheme)) = graphemes.get(end) {
                let width = display_width(grapheme);
                // Leading whitespace is literal indentation, not an empty word.
                if matches!(options.mode, WrapMode::Word)
                    && is_break_space(grapheme)
                    && end > start
                    && !is_break_space(graphemes[end - 1].1)
                {
                    word_break = Some(end);
                }
                if end > start && columns.saturating_add(width) > available {
                    break;
                }
                columns = columns.saturating_add(width);
                end += 1;
            }
            let mut next = end;
            if end < graphemes.len()
                && matches!(options.mode, WrapMode::Word)
                && let Some(boundary) = word_break
            {
                end = boundary;
                next = boundary;
                while next < graphemes.len() && is_break_space(graphemes[next].1) {
                    next += 1;
                }
            }
            let byte_start = graphemes[start].0;
            let byte_end = graphemes.get(end).map_or(line.len(), |(offset, _)| *offset);
            rows.push(line_start + byte_start..line_start + byte_end);
            start = next;
        }
        line_start += line.len() + 1;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(input: &str, width: usize, mode: WrapMode) -> Vec<&str> {
        wrap_ranges(
            input,
            WrapOptions {
                width,
                initial_indent: 0,
                subsequent_indent: 0,
                mode,
            },
        )
        .into_iter()
        .map(|range| &input[range])
        .collect()
    }

    #[test]
    fn word_boundaries_do_not_create_empty_rows_or_leading_soft_spaces() {
        for width in [4, 5, 8] {
            assert_eq!(
                rows("aaaa bbbb cccc", width, WrapMode::Word),
                ["aaaa", "bbbb", "cccc"]
            );
        }
        assert_eq!(
            rows("aaaa bbbb cccc", 9, WrapMode::Word),
            ["aaaa bbbb", "cccc"]
        );
        assert_eq!(rows("  aa bb", 5, WrapMode::Word), ["  aa", "bb"]);
        assert_eq!(rows("a     bb", 4, WrapMode::Word), ["a", "bb"]);
        assert_eq!(rows("x aa\u{a0}bb", 5, WrapMode::Word), ["x", "aa\u{a0}bb"]);
        assert_eq!(
            rows("x aa\u{202f}bb", 5, WrapMode::Word),
            ["x", "aa\u{202f}bb"]
        );
    }

    #[test]
    fn explicit_empty_lines_and_trailing_newlines_are_preserved() {
        for mode in [WrapMode::Word, WrapMode::Grapheme] {
            assert_eq!(rows("\nabc\n\n", 3, mode), ["", "abc", "", ""]);
            assert_eq!(rows("", 0, mode), [""]);
        }
    }

    #[test]
    fn oversized_tokens_split_only_at_grapheme_boundaries() {
        let joined = "\u{1f469}\u{200d}\u{1f4bb}";
        let flag = "\u{1f1f8}\u{1f1ec}";
        for mode in [WrapMode::Word, WrapMode::Grapheme] {
            assert_eq!(
                rows("a\u{301}b\u{301}c", 1, mode),
                ["a\u{301}", "b\u{301}", "c"]
            );
            assert_eq!(rows(&format!("{joined}{flag}"), 2, mode), [joined, flag]);
            assert_eq!(rows("abcdefg", 3, mode), ["abc", "def", "g"]);
            assert_eq!(rows("\u{754c}\u{754c}", 1, mode), ["\u{754c}", "\u{754c}"]);
        }
    }

    #[test]
    fn shared_width_policy_includes_tab_expansion() {
        assert_eq!(rows("a\tb", 5, WrapMode::Grapheme), ["a\t", "b"]);
        assert_eq!(expand_tabs("a\tb"), "a    b");
        assert_eq!(grapheme_width("\u{1f469}\u{200d}\u{1f4bb}"), 2);
        assert_eq!(grapheme_width("a\u{301}"), 1);
    }

    #[test]
    fn prefix_and_suffix_clipping_preserve_unicode_boundaries() {
        let source = "a\u{301}\u{754c}\u{1f469}\u{200d}\u{1f4bb}\u{ff76}\u{ff9e}";
        let boundaries = source
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .chain([source.len()])
            .collect::<Vec<_>>();
        for width in 0..16 {
            let prefix = truncate_to_width(source, width);
            let suffix = suffix_to_width(source, width);
            assert!(boundaries.contains(&prefix.len()));
            assert!(boundaries.contains(&(source.len() - suffix.len())));
            assert!(display_width(prefix) <= width);
            assert!(display_width(suffix) <= width);
        }
    }
}
