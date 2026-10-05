use super::text_wrap::display_width;

/// Keep the existing cell width so cursor, wrapping, and source selection agree.
pub(super) fn ascii_cell(symbol: &str) -> String {
    let width = display_width(symbol);
    if width == 0 {
        return String::new();
    }
    let first = symbol.chars().next().unwrap_or(' ');
    let replacement = match first {
        '\u{2500}' | '\u{2501}' | '\u{2504}' | '\u{2505}' | '\u{2508}' | '\u{2509}'
        | '\u{254c}' | '\u{254d}' | '\u{2550}' => '-',
        '\u{2502}' | '\u{2503}' | '\u{2506}' | '\u{2507}' | '\u{250a}' | '\u{250b}'
        | '\u{254e}' | '\u{254f}' | '\u{2551}' => '|',
        '\u{2500}'..='\u{257f}' => '+',
        '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
        '\u{2190}' | '\u{2039}' | '\u{25c0}' | '\u{25c2}' => '<',
        '\u{2192}' | '\u{203a}' | '\u{25b6}' | '\u{25b8}' => '>',
        '\u{2191}' | '\u{25b2}' | '\u{25b4}' => '^',
        '\u{2193}' | '\u{25bc}' | '\u{25be}' => 'v',
        '\u{2026}' => '.',
        '\u{2022}' | '\u{00b7}' | '\u{25cf}' | '\u{25aa}' | '\u{2728}' => '*',
        '\u{25cb}' | '\u{25e6}' | '\u{2610}' => 'o',
        '\u{2713}' | '\u{2714}' | '\u{2611}' => '+',
        '\u{2717}' | '\u{2718}' | '\u{2612}' | '\u{274c}' => 'x',
        '\u{26a0}' => '!',
        '\u{2580}'..='\u{2590}' | '\u{2593}'..='\u{259f}' | '\u{25a0}' => '#',
        '\u{2591}' => '.',
        '\u{2592}' => ':',
        '\u{2800}'..='\u{28ff}' => ['|', '/', '-', '\\'][(first as usize) % 4],
        '\u{27e6}' => '[',
        '\u{27e7}' => ']',
        '\u{2018}' | '\u{2019}' => '\'',
        '\u{201c}' | '\u{201d}' => '"',
        '\u{00a0}' => ' ',
        _ => '?',
    };
    let mut result = String::with_capacity(width);
    result.push(replacement);
    result.extend(std::iter::repeat_n(
        if replacement == '?' { '?' } else { ' ' },
        width - 1,
    ));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_table_preserves_width_for_ui_and_unknown_clusters() {
        for (input, expected) in [
            ("\u{25b8}", ">"),
            ("\u{25cf}", "*"),
            ("\u{2713}", "+"),
            ("\u{2610}", "o"),
            ("\u{2026}", "."),
            ("\u{2514}", "+"),
            ("\u{2500}", "-"),
            ("\u{2502}", "|"),
            ("\u{754c}", "??"),
            ("e\u{301}", "?"),
            ("\u{1f469}\u{200d}\u{1f4bb}", "??"),
        ] {
            let ascii = ascii_cell(input);
            assert_eq!(ascii, expected);
            assert_eq!(display_width(&ascii), display_width(input));
            assert!(ascii.is_ascii());
        }
    }
}
