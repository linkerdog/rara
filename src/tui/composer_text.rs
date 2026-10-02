use std::cell::RefCell;
use std::sync::Arc;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TAB_WIDTH: usize = 4;
pub(crate) const COMPOSER_INITIAL_INDENT: &str = "› ";
pub(crate) const COMPOSER_SUBSEQUENT_INDENT: &str = "  ";

pub(crate) struct WrapConfig<'a> {
    pub width: u16,
    pub initial_indent: &'a str,
    pub subsequent_indent: &'a str,
}

impl WrapConfig<'static> {
    pub(crate) fn composer(width: u16) -> Self {
        Self {
            width,
            initial_indent: COMPOSER_INITIAL_INDENT,
            subsequent_indent: COMPOSER_SUBSEQUENT_INDENT,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct VisualPosition {
    pub row: usize,
    pub column: usize,
}

/// Plain display rows and character-offset mapping shared by editing and rendering.
pub(crate) struct WrappedText {
    rows: Vec<String>,
    positions: Vec<VisualPosition>,
    width: usize,
}

impl WrappedText {
    pub(crate) fn rows(&self) -> &[String] {
        &self.rows
    }

    pub(crate) fn cursor_position(&self, offset: usize) -> VisualPosition {
        let mut position = self.position_for_offset(offset);
        position.column = position.column.min(self.width - 1);
        position
    }

    /// Keeps insertion-boundary columns distinct before hardware cursor clipping.
    pub(crate) fn position_for_offset(&self, offset: usize) -> VisualPosition {
        self.positions[offset.min(self.positions.len() - 1)]
    }

    pub(crate) fn offset_for_position(&self, target: VisualPosition) -> usize {
        if target.row >= self.rows.len() {
            return self.positions.len() - 1;
        }
        self.positions
            .iter()
            .enumerate()
            .filter(|(_, position)| position.row == target.row)
            .min_by_key(|(offset, position)| {
                (
                    position.column.abs_diff(target.column),
                    std::cmp::Reverse(*offset),
                )
            })
            .map_or(0, |(offset, _)| offset)
    }
}

struct WrapCache {
    input: String,
    width: u16,
    initial_indent: String,
    subsequent_indent: String,
    layout: Arc<WrappedText>,
}

pub(crate) fn wrapped_text(input: &str, config: WrapConfig<'_>) -> Arc<WrappedText> {
    thread_local! {
        static CACHE: RefCell<Option<WrapCache>> = const { RefCell::new(None) };
    }
    CACHE.with(|cell| {
        if let Some(cache) = cell.borrow().as_ref()
            && cache.input == input
            && cache.width == config.width
            && cache.initial_indent == config.initial_indent
            && cache.subsequent_indent == config.subsequent_indent
        {
            return cache.layout.clone();
        }
        let layout = Arc::new(build_layout(input, &config));
        cell.replace(Some(WrapCache {
            input: input.into(),
            width: config.width,
            initial_indent: config.initial_indent.into(),
            subsequent_indent: config.subsequent_indent.into(),
            layout: layout.clone(),
        }));
        layout
    })
}

fn build_layout(input: &str, config: &WrapConfig<'_>) -> WrappedText {
    let width = usize::from(config.width.max(1));
    let mut rows = Vec::new();
    let mut current = config.initial_indent.to_string();
    let mut prefix_width = UnicodeWidthStr::width(config.initial_indent).min(width);
    let mut column = prefix_width;
    let mut positions = vec![VisualPosition { row: 0, column }];
    for ch in input.chars() {
        if ch == '\n' {
            rows.push(current);
            current = config.subsequent_indent.to_string();
            prefix_width = UnicodeWidthStr::width(config.subsequent_indent).min(width);
            column = prefix_width;
        } else {
            let char_width = match ch {
                '\t' => TAB_WIDTH,
                _ => UnicodeWidthChar::width(ch).unwrap_or(0),
            };
            if column.saturating_add(char_width) > width && column > prefix_width {
                rows.push(current);
                current = config.subsequent_indent.to_string();
                prefix_width = UnicodeWidthStr::width(config.subsequent_indent).min(width);
                column = prefix_width;
                // This character starts the next row, so its preceding cursor
                // boundary must move with it instead of staying on the old row.
                *positions.last_mut().expect("initial cursor boundary") = VisualPosition {
                    row: rows.len(),
                    column,
                };
            }
            current.push(ch);
            column = column.saturating_add(char_width);
        }
        positions.push(VisualPosition {
            row: rows.len(),
            column,
        });
    }
    rows.push(current);
    WrappedText {
        rows,
        positions,
        width,
    }
}

pub(crate) fn expand_tabs(text: &str) -> String {
    text.replace('\t', &" ".repeat(TAB_WIDTH))
}

/// Measures the displayed prefix of a clipped, single-line editor.
pub(crate) fn clipped_cursor_column(input: &str, offset: usize, width: u16) -> usize {
    let prefix = input
        .chars()
        .take(offset)
        .take_while(|ch| *ch != '\n')
        .collect::<String>();
    UnicodeWidthStr::width(expand_tabs(&prefix).as_str()).min(usize::from(width.max(1) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_wrap_boundary_belongs_to_the_following_character_row() {
        let layout = wrapped_text("abcdefg", WrapConfig::composer(6));
        assert_eq!(layout.rows(), &["› abcd", "  efg"]);
        let position = layout.cursor_position(4);
        assert_eq!((position.row, position.column), (1, 2));
        assert_eq!(layout.offset_for_position(position), 4);
        let full_final_row = wrapped_text("abcd", WrapConfig::composer(6));
        let end = full_final_row.cursor_position(4);
        assert_eq!((end.row, end.column), (0, 5));
    }

    #[test]
    fn blank_lines_tabs_and_wide_characters_share_display_positions() {
        let layout = wrapped_text("\t12\n\n\u{754c}x", WrapConfig::composer(8));
        assert_eq!(layout.rows(), &["› \t12", "  ", "  \u{754c}x"]);
        for (offset, expected) in [(1, (0, 6)), (4, (1, 2)), (5, (2, 2)), (6, (2, 4))] {
            let position = layout.cursor_position(offset);
            assert_eq!((position.row, position.column), expected);
            assert_eq!(layout.offset_for_position(position), offset);
        }
        assert_eq!(expand_tabs("\t\u{754c}"), "    \u{754c}");
    }

    #[test]
    fn matching_layouts_share_the_cache_but_width_and_text_changes_do_not() {
        let first = wrapped_text("cache this draft", WrapConfig::composer(12));
        let same = wrapped_text("cache this draft", WrapConfig::composer(12));
        assert!(Arc::ptr_eq(&first, &same));
        let resized = wrapped_text("cache this draft", WrapConfig::composer(13));
        assert!(!Arc::ptr_eq(&first, &resized));
        let changed = wrapped_text("cache the new draft", WrapConfig::composer(13));
        assert!(!Arc::ptr_eq(&resized, &changed));
    }

    #[test]
    fn full_row_insertion_boundaries_remain_distinct_when_the_cursor_is_clipped() {
        for input in ["abcdefgh", "abcd\nefgh\n"] {
            let layout = wrapped_text(input, WrapConfig::composer(6));
            let end = if input.contains('\n') { 9 } else { 8 };
            for offset in [end - 1, end] {
                assert_eq!(
                    layout.offset_for_position(layout.position_for_offset(offset)),
                    offset
                );
            }
            assert_eq!(
                layout.cursor_position(end - 1).column,
                layout.cursor_position(end).column
            );
            assert_ne!(
                layout.position_for_offset(end - 1).column,
                layout.position_for_offset(end).column
            );
        }
    }

    #[test]
    fn clipped_editors_measure_display_columns_without_soft_wrapping() {
        assert_eq!(clipped_cursor_column("abcdef", 4, 4), 3);
        assert_eq!(clipped_cursor_column("abcdef", 6, 4), 3);
        assert_eq!(clipped_cursor_column("\u{754c}\tx", 2, 10), 6);
        assert_eq!(clipped_cursor_column("abcdef", 4, 0), 0);
    }
}
