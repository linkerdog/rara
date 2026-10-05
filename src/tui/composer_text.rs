use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use super::display_sanitize::{annotate_bidi_text, bidi_annotation};
pub(crate) use super::text_wrap::expand_tabs;
use super::text_wrap::{WrapMode, WrapOptions, display_width, grapheme_width, wrap_ranges};

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
    positions: Vec<(usize, VisualPosition)>,
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
        let index = self
            .positions
            .partition_point(|(boundary, _)| *boundary <= offset);
        self.positions[index.saturating_sub(1)].1
    }

    pub(crate) fn offset_for_position(&self, target: VisualPosition) -> usize {
        if target.row >= self.rows.len() {
            return self.positions.last().map_or(0, |(offset, _)| *offset);
        }
        self.positions
            .iter()
            .min_by_key(|(offset, position)| {
                (
                    position.row.abs_diff(target.row),
                    position.column.abs_diff(target.column),
                    std::cmp::Reverse(*offset),
                )
            })
            .map_or(0, |(offset, _)| *offset)
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
    let projected = annotate_bidi_text(input);
    let mut layout = build_display_layout(&projected, config);
    if matches!(projected, Cow::Borrowed(_)) {
        return layout;
    }

    // Only source grapheme boundaries are editable. Intermediate columns/rows
    // inside an expanded label must never become fictitious source offsets.
    let mut source_offset = 0;
    let mut display_offset = 0;
    let mut positions = vec![(0, layout.position_for_offset(0))];
    for grapheme in input.graphemes(true) {
        for ch in grapheme.chars() {
            source_offset += 1;
            display_offset += bidi_annotation(ch).map_or(1, |label| label.chars().count());
        }
        positions.push((source_offset, layout.position_for_offset(display_offset)));
    }
    layout.positions = positions;
    layout
}

/// Wraps already projected text; offsets here belong to that display string.
fn build_display_layout(input: &str, config: &WrapConfig<'_>) -> WrappedText {
    let width = usize::from(config.width.max(1));
    let mut rows = Vec::new();
    let ranges = wrap_ranges(
        input,
        WrapOptions {
            width,
            initial_indent: display_width(config.initial_indent).min(width),
            subsequent_indent: display_width(config.subsequent_indent).min(width),
            mode: WrapMode::Grapheme,
        },
    );
    let mut positions: Vec<(usize, VisualPosition)> = Vec::new();
    let mut byte_offset = 0;
    let mut offset = 0;
    for (row, range) in ranges.into_iter().enumerate() {
        offset += input[byte_offset..range.start].chars().count();
        let indent = if row == 0 {
            config.initial_indent
        } else {
            config.subsequent_indent
        };
        let mut column = display_width(indent).min(width);
        let start = VisualPosition { row, column };
        if let Some(last) = positions.last_mut()
            && last.0 == offset
        {
            last.1 = start;
        } else {
            positions.push((offset, start));
        }
        let text = &input[range.clone()];
        for grapheme in text.graphemes(true) {
            offset += grapheme.chars().count();
            column = column.saturating_add(grapheme_width(grapheme));
            positions.push((offset, VisualPosition { row, column }));
        }
        rows.push(format!("{indent}{text}"));
        byte_offset = range.end;
    }
    WrappedText {
        rows,
        positions,
        width,
    }
}

/// Measures the displayed prefix of a clipped, single-line editor.
pub(crate) fn clipped_cursor_column(input: &str, offset: usize, width: u16) -> usize {
    let offset = super::input_text::floor_grapheme_offset(input, offset);
    let prefix = input
        .chars()
        .take(offset)
        .take_while(|ch| *ch != '\n')
        .collect::<String>();
    display_width(&annotate_bidi_text(&prefix)).min(usize::from(width.max(1) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_cursor_never_measures_an_interior_emoji_prefix() {
        let input = "\u{1f469}\u{200d}\u{1f4bb}z";
        assert_eq!(clipped_cursor_column(input, 1, 8), 0);
        assert_eq!(clipped_cursor_column(input, 2, 8), 0);
        assert_eq!(clipped_cursor_column(input, 3, 8), 2);
    }

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

    #[test]
    fn cursor_geometry_uses_whole_graphemes_without_changing_character_offsets() {
        let input = "a\u{301}\u{1f469}\u{200d}\u{1f4bb}z";
        let layout = wrapped_text(input, WrapConfig::composer(5));
        assert_eq!(
            layout.rows(),
            &["› a\u{301}\u{1f469}\u{200d}\u{1f4bb}", "  z"]
        );
        for (offset, expected) in [
            (0, (0, 2)),
            (1, (0, 2)),
            (2, (0, 3)),
            (3, (0, 3)),
            (4, (0, 3)),
            (5, (1, 2)),
            (6, (1, 3)),
        ] {
            let position = layout.position_for_offset(offset);
            assert_eq!((position.row, position.column), expected, "offset {offset}");
        }
        assert_eq!(
            layout.offset_for_position(VisualPosition { row: 0, column: 4 }),
            2
        );
        assert_eq!(
            layout.offset_for_position(VisualPosition { row: 1, column: 2 }),
            5
        );
    }
}
