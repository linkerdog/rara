//! Grapheme boundaries expressed in the editor's existing character-offset units.

use unicode_segmentation::UnicodeSegmentation;

fn boundaries(text: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(text.graphemes(true).scan(0, |offset, grapheme| {
        *offset += grapheme.chars().count();
        Some(*offset)
    }))
}

pub(crate) fn floor_grapheme_offset(text: &str, offset: usize) -> usize {
    boundaries(text)
        .take_while(|&boundary| boundary <= offset)
        .last()
        .unwrap_or(0)
}

pub(crate) fn ceil_grapheme_offset(text: &str, offset: usize) -> usize {
    boundaries(text)
        .find(|&boundary| boundary >= offset)
        .unwrap_or_else(|| text.chars().count())
}

pub(crate) fn previous_grapheme_offset(text: &str, offset: usize) -> usize {
    boundaries(text)
        .take_while(|&boundary| boundary < offset)
        .last()
        .unwrap_or(0)
}

pub(crate) fn next_grapheme_offset(text: &str, offset: usize) -> usize {
    boundaries(text)
        .find(|&boundary| boundary > offset)
        .unwrap_or_else(|| text.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_offsets_snap_without_changing_text() {
        let text = "a\u{301}\u{1f469}\u{200d}\u{1f4bb}\u{754c}";
        let expected = [0, 2, 5, 6];
        assert_eq!(boundaries(text).collect::<Vec<_>>(), expected);
        for offset in 0..=8 {
            assert_eq!(
                floor_grapheme_offset(text, offset),
                *expected.iter().rfind(|&&b| b <= offset).expect("start")
            );
            assert_eq!(
                ceil_grapheme_offset(text, offset),
                expected.iter().copied().find(|&b| b >= offset).unwrap_or(6)
            );
            assert_eq!(
                previous_grapheme_offset(text, offset),
                expected.iter().copied().rfind(|&b| b < offset).unwrap_or(0)
            );
            assert_eq!(
                next_grapheme_offset(text, offset),
                expected.iter().copied().find(|&b| b > offset).unwrap_or(6)
            );
        }
    }
}
