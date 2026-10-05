#![expect(
    clippy::print_stderr,
    reason = "Expose deterministic work counts for formatted paragraph continuations."
)]

use super::MarkdownStreamCollector;
use crate::tui::markdown_render::render_markdown_text_with_width_and_cwd;

#[test]
fn formatted_paragraph_ordinary_continuation_has_linear_source_work() {
    for chunk in [
        "ordinary words ",
        "Words 👩‍💻 ",
        "Words cafe\u{301} ",
        "Words finished. ",
    ] {
        let mut measurements = Vec::new();
        for prefix in [
            "**Important:** ",
            "Use `value` for this: ",
            "See [guide](https://example.com) and continue: ",
            "Text with an &amp; entity: ",
            "Unclosed *marker with ordinary words: ",
        ] {
            let cwd = std::env::temp_dir();
            let mut stream = MarkdownStreamCollector::new(None, &cwd);
            let mut source = prefix.to_string();
            stream.push_delta(prefix);
            stream.lines();
            for _ in 0..200 {
                source.push_str(chunk);
                stream.push_delta(chunk);
                stream.lines();
            }
            assert_eq!(
                stream.lines(),
                render_markdown_text_with_width_and_cwd(&source, None, Some(&cwd)).lines,
                "{prefix:?}"
            );
            let work = stream.work();
            eprintln!(
                "{prefix:?}, {chunk:?}: {} source bytes, {work:?}",
                source.len()
            );
            measurements.push((prefix, source.len(), work));
        }
        for (prefix, bytes, work) in measurements {
            assert!(work.parsed_bytes <= bytes * 8, "{prefix:?}: {work:?}");
            assert!(work.plain_bytes <= bytes * 4, "{prefix:?}: {work:?}");
        }
    }
}

#[test]
fn formatted_continuations_match_canonical_at_every_character_and_split() {
    for prefix in [
        "**Important:** ",
        "_Italic_ ",
        "~~Old~~ ",
        "Use `value` ",
        "See [guide](https://example.com) ",
        "See [local](src/lib.rs) ",
        "A &amp; ",
        "**Formatted** 👩‍💻 ",
        "**Formatted** cafe\u{301} ",
        "**Formatted** words. ",
        "**Formatted** words, ",
        "**Formatted** words! ",
        "**Formatted** words? ",
        "**Formatted** words; ",
        "A &#32; ",
        "A \\* ",
        "A ] ",
        "A * ",
        "Unclosed *marker ",
        "Unclosed `code ",
        "A [label](target ",
        "A [label](target \"title ",
        "A <span ",
        "[id]: ",
        "1. ",
        "Plain\n1. ",
        "**Formatted**\n1. ",
        "Plain\n+ ",
        "Plain\n- ",
        "Plain\n* ",
        "Plain\n  1. ",
        "  [id]: ",
        "[id]: target \"title ",
        "A\n  ",
        "A\t ",
        "# Heading\n\n**Important:** ",
        "> **Quoted** ",
        "- **Item** ",
        "See ![image](path) ",
        "[id]: target\n\nSee [id] ",
    ] {
        for suffix in [
            "ordinary words more words ",
            "ordinary words\nNext words\nLast words",
            "ordinary words  \nNext words",
            "ordinary words *close* `code` [link](path)",
            "ordinary words ) closing\"title\" ",
            "ordinary words\n===\nAfter heading",
            "ordinary words\n\n[id]: destination\nAfter [id]",
            "ordinary words\n:12 and more",
            "words 👩‍💻 cafe\u{301} 🇬🇧 \u{202e}tail",
        ] {
            let source = format!("{prefix}{suffix}");
            assert_every_character(&source);
            for (split, _) in source.char_indices() {
                let cwd = std::env::temp_dir();
                let mut stream = MarkdownStreamCollector::new(None, &cwd);
                stream.push_delta(&source[..split]);
                stream.lines();
                stream.push_delta(&source[split..]);
                assert_eq!(
                    stream.lines(),
                    render_markdown_text_with_width_and_cwd(&source, None, Some(&cwd)).lines,
                    "split {split} in {source:?}"
                );
            }
        }
    }
}

fn assert_every_character(source: &str) {
    let cwd = std::env::temp_dir();
    let mut stream = MarkdownStreamCollector::new(None, &cwd);
    for (offset, ch) in source.char_indices() {
        let end = offset + ch.len_utf8();
        stream.push_delta(&source[offset..end]);
        assert_eq!(
            stream.lines(),
            render_markdown_text_with_width_and_cwd(&source[..end], None, Some(&cwd)).lines,
            "prefix {:?} of {source:?}",
            &source[..end]
        );
    }
}

#[test]
fn formatted_continuation_ascii_boundaries_match_canonical() {
    for prefix in [
        "**bold** ordinary ",
        "A [label](target ordinary ",
        "A &amp; ordinary ",
    ] {
        for byte in 0..=127u8 {
            assert_every_character(&format!("{prefix}{}{byte} end", char::from(byte)));
        }
    }
}
