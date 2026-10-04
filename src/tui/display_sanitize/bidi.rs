use std::borrow::Cow;

/// The Unicode Bidi_Control set, deliberately excluding Join_Control and
/// variation selectors. Labels have no Markdown delimiter semantics.
pub(crate) fn bidi_annotation(ch: char) -> Option<&'static str> {
    match ch {
        '\u{061c}' => Some("⟦U+061C⟧"),
        '\u{200e}' => Some("⟦U+200E⟧"),
        '\u{200f}' => Some("⟦U+200F⟧"),
        '\u{202a}' => Some("⟦U+202A⟧"),
        '\u{202b}' => Some("⟦U+202B⟧"),
        '\u{202c}' => Some("⟦U+202C⟧"),
        '\u{202d}' => Some("⟦U+202D⟧"),
        '\u{202e}' => Some("⟦U+202E⟧"),
        '\u{2066}' => Some("⟦U+2066⟧"),
        '\u{2067}' => Some("⟦U+2067⟧"),
        '\u{2068}' => Some("⟦U+2068⟧"),
        '\u{2069}' => Some("⟦U+2069⟧"),
        _ => None,
    }
}

/// Project editable source without mutating submitted text or ordinary clusters.
pub(crate) fn annotate_bidi_text(input: &str) -> Cow<'_, str> {
    let Some((first, _)) = input
        .char_indices()
        .find(|(_, ch)| bidi_annotation(*ch).is_some())
    else {
        return Cow::Borrowed(input);
    };
    let mut output = String::with_capacity(input.len());
    output.push_str(&input[..first]);
    for ch in input[first..].chars() {
        if let Some(label) = bidi_annotation(ch) {
            output.push_str(label);
        } else {
            output.push(ch);
        }
    }
    Cow::Owned(output)
}
