#![expect(
    clippy::print_stderr,
    reason = "Work-count regression tests expose measured counts in test diagnostics."
)]

use super::*;
use crate::tui::markdown_render::render_markdown_text_with_width_and_cwd;

fn collector() -> MarkdownStreamCollector {
    MarkdownStreamCollector::new(None, &std::env::temp_dir())
}

fn canonical(source: &str, width: Option<usize>) -> Vec<Line<'static>> {
    render_markdown_text_with_width_and_cwd(source, width, Some(&std::env::temp_dir())).lines
}

fn assert_live_equals_full(chunks: &[&str]) {
    let mut stream = collector();
    let mut source = String::new();
    for chunk in chunks {
        source.push_str(chunk);
        stream.push_delta(chunk);
        assert_eq!(stream.lines(), canonical(&source, None), "after {chunk:?}");
    }
    stream.finalize();
    assert_eq!(stream.lines(), canonical(&source, None));
}

#[test]
fn fence_previews_do_not_advance_multiline_syntax_state() {
    assert_live_equals_full(&[
        "```rust\n",
        "/* start\n",
        "comment",
        " continues",
        "\n",
        "end */\n",
        "let",
        " value = 1",
        ";",
        "\n",
        "```",
        "\n\nAfter.",
    ]);
}

#[test]
fn open_fence_empty_rows_match_canonical_spans() {
    for marker in ["```", "~~~"] {
        for language in ["", "unknown-language", "rust"] {
            let first = format!("Intro\n\n{marker}{language}\ncode\n");
            let last = format!("next\n{marker}\n\nAfter.");
            assert_live_equals_full(&[&first, "\n", "\n", &last]);
        }
    }
}

#[test]
fn fence_and_nested_blocks_match_canonical_at_every_split() {
    let sources = [
        "Intro\n\n```\ncode\n\nnext\n```\n\nAfter.",
        "Intro\r\n\r\n~~~rust\r\nlet x = 1;\r\n\r\n~~~\r\nAfter.",
        "- first\n\n  ```\n  code\n\n  next\n  ```\n\nAfter.",
        "> quote\n>\n> ```\n> code\n>\n> next\n> ```\n\nAfter.",
    ];
    for source in sources {
        for (split, _) in source.char_indices().chain([(source.len(), '\0')]) {
            assert_live_equals_full(&[&source[..split], &source[split..]]);
        }
    }
}

#[test]
fn fence_candidates_and_normalization_keep_canonical_fallback() {
    for chunks in [
        vec![
            "```rust\n",
            "let a = 1;\n",
            "``",
            "`not a closer\n",
            "```\n",
        ],
        vec![
            "~~~~rust\n",
            "/* comment\n",
            "text\n",
            "*/\n",
            "~~~\n",
            "~~~~\n",
        ],
        vec!["  ```rust\n", "  let a = 1;\n", "  ```\n"],
        vec!["> ```rust\n", "> let a = 1;\n", "> ```\n"],
        vec!["```ru\\st\n", "let a = 1;\n", "```\n"],
        vec!["```rust\n", "a\0b\n", "more\n", "```\n"],
        vec!["```rust\n", "a\r\nb\r\n", "more\n", "```\n"],
        vec!["Before.\n\n", "```rust\n", "```\n\n", "After.\n"],
    ] {
        assert_live_equals_full(&chunks);
    }
}

#[test]
fn fence_highlight_byte_limit_recolors_retained_rows_once() {
    let mut stream = collector();
    let first = "let value = 1;\n";
    let padding = format!("//{}\n", "x".repeat(512 * 1024 - first.len() - 20));
    let mut source = format!("```rust\n{first}{padding}");
    stream.push_delta(&source);
    stream.lines();
    source.push_str("// crosses the highlight limit\n");
    stream.push_delta("// crosses the highlight limit\n");
    assert_eq!(stream.lines(), canonical(&source, None));
    let work = stream.work();
    for _ in 0..10 {
        source.push_str(first);
        stream.push_delta(first);
        stream.lines();
    }
    assert_eq!(stream.work().parses, work.parses);
    assert_eq!(stream.lines(), canonical(&source, None));
}

#[test]
fn incomplete_line_is_a_replaceable_preview() {
    let mut stream = collector();
    stream.push_delta("Hello");
    assert_eq!(stream.lines(), canonical("Hello", None));
    assert_eq!(stream.stable_source_len, 0);
    stream.push_delta(" world\n");
    assert_eq!(stream.lines(), canonical("Hello world\n", None));
}

#[test]
fn preview_includes_only_one_copy_of_complete_and_partial_lines() {
    assert_live_equals_full(&["Hello\nWorld", "\nAgain", " and again"]);
}

#[test]
fn finalize_commits_partial_line() {
    let mut stream = collector();
    stream.push_delta("Line without newline");
    stream.finalize();
    assert_eq!(stream.lines(), canonical("Line without newline", None));
}

#[test]
fn completed_blocks_have_linear_parse_bytes() {
    let mut stream = collector();
    let mut source = String::new();
    for _ in 0..200 {
        let chunk = "A completed paragraph.\n\n";
        source.push_str(chunk);
        stream.push_delta(chunk);
        stream.lines();
    }
    let work = stream.work();
    assert!(work.parsed_bytes <= source.len() * 8, "{work:?}");
    eprintln!("completed blocks: {work:?}");
    assert!(work.rendered_rows <= 200 * 8, "{work:?}");
    assert_eq!(stream.lines(), canonical(&source, None));
}

#[test]
fn unchanged_presentation_does_not_parse_again() {
    let mut stream = collector();
    stream.push_delta("First paragraph.\n\nSecond paragraph.");
    stream.lines();
    let work = stream.work();
    let row_address = stream.lines().as_ptr();
    for _ in 0..100 {
        assert_eq!(stream.lines().as_ptr(), row_address);
    }
    assert_eq!(stream.work(), work);
}

#[test]
fn final_table_matches_canonical_styled_rows() {
    let mut stream = collector();
    let chunks = [
        "Intro.\n\n",
        "| A | B |\n",
        "| --- | --- |\n",
        "| longer | value |\n",
    ];
    for chunk in chunks {
        stream.push_delta(chunk);
        stream.lines();
    }
    stream.finalize();
    assert_eq!(stream.lines(), canonical(&chunks.concat(), None));
}

#[test]
fn table_interrupting_a_mutable_paragraph_preserves_preceding_prose_at_every_split() {
    for before in [
        "Intro.\n",
        "Stable.\n\nA mutable **paragraph**.\nContinued prose.\n",
        "Stable.\r\n\r\nUnicode \u{4e16}\u{754c} e\u{301}.\r\n",
        "See [id].\n\n[id]: https://example.com\n\nMore prose.\n",
    ] {
        let source = format!("{before}| A | B |\n| --- | --- |\n| long | value |\n\nAfter.\n");
        let expected_live = canonical(before, None);
        let expected_final = canonical(&source, None);
        for (split, _) in source.char_indices().chain([(source.len(), '\0')]) {
            let mut stream = collector();
            for chunk in [&source[..split], &source[split..]] {
                stream.push_delta(chunk);
                stream.lines();
            }
            assert!(
                stream.held_table_start.is_some(),
                "split {split}: {source:?}"
            );
            assert_eq!(stream.lines(), expected_live, "split {split}: {source:?}");
            stream.finalize();
            assert_eq!(stream.lines(), expected_final, "split {split}: {source:?}");
        }
    }
}

#[test]
fn loose_list_matches_canonical_styled_rows() {
    assert_live_equals_full(&["- first\n", "- second\n\n", "  another paragraph\n"]);
}

#[test]
fn ingestion_does_not_parse_or_render() {
    let mut stream = collector();
    for _ in 0..1000 {
        stream.push_delta("word ");
    }
    assert_eq!(stream.work().parses, 0);
    assert_eq!(stream.work().rendered_rows, 0);
    assert_eq!(stream.work().appended_bytes, 5000);
    assert_eq!(stream.lines(), canonical(&"word ".repeat(1000), None));
    assert_eq!(stream.work().parses, 1);
}

#[test]
fn stable_rows_keep_their_owned_text_allocation() {
    let mut stream = collector();
    stream.push_delta("Stable paragraph.\n\nMutable paragraph.\n\n");
    let first_text = stream.lines()[0].spans[0].content.as_ptr();
    assert!(stream.stable_source_len > 0);
    for _ in 0..100 {
        stream.push_delta("Another paragraph.\n\n");
        assert_eq!(stream.lines()[0].spans[0].content.as_ptr(), first_text);
    }
}

#[test]
fn confirmed_table_and_following_source_are_held_until_finalize() {
    let mut stream = collector();
    stream.push_delta("Intro.\n\n| A | B |\n| --- | --- |\n");
    assert_eq!(stream.lines(), canonical("Intro.\n\n", None));
    let work = stream.work();
    let mut source = stream.buffer.clone();
    for _ in 0..500 {
        let chunk = "| longer | value |\n";
        source.push_str(chunk);
        stream.push_delta(chunk);
        assert_eq!(stream.lines(), canonical("Intro.\n\n", None));
    }
    stream.push_delta("\nAfter the table.");
    source.push_str("\nAfter the table.");
    assert_eq!(stream.work().parses, work.parses);
    stream.finalize();
    assert_eq!(stream.lines(), canonical(&source, None));
    assert_eq!(stream.work().parses, work.parses + 1);
}

#[test]
fn partial_heading_does_not_freeze_a_paragraph_boundary() {
    assert_live_equals_full(&["First line\n", "#", "not a heading\n", "more text"]);
}

#[test]
fn incomplete_delimiter_cannot_permanently_hold_a_non_table() {
    assert_live_equals_full(&[
        "Intro.\n\n",
        "| A | B |\n",
        "| --- | ---",
        "oops |\n",
        "\nAfter.",
    ]);
}

#[test]
fn reference_fallback_still_holds_completed_tables() {
    let mut stream = collector();
    let before = "[id]: https://example.com\n\nSee [id].\n\n";
    let source = format!("{before}| A | B |\n| --- | --- |\n| long | value |\n");
    stream.push_delta(&source);
    assert_eq!(stream.lines(), canonical(before, None));
    assert!(stream.held_table_start.is_some());
    stream.finalize();
    assert_eq!(stream.lines(), canonical(&source, None));
}

#[test]
fn setext_heading_replaces_earlier_paragraph_style() {
    assert_live_equals_full(&["Heading\n", "---", "\n", "\nAfter.\n"]);
}

#[test]
fn reference_definitions_invalidate_prior_blocks() {
    assert_live_equals_full(&[
        "See [docs][id].\n\n",
        "Another paragraph.\n\n",
        "[id]: https://example.com\n",
        "\nLater [id].\n",
    ]);
}

#[test]
fn reference_context_keeps_completed_paragraph_work_linear() {
    let mut stream = collector();
    let mut source = "[id]: https://example.com\n\n".to_string();
    stream.push_delta(&source);
    stream.lines();
    for _ in 0..200 {
        let chunk = "See [the docs][id].\n\n";
        source.push_str(chunk);
        stream.push_delta(chunk);
        assert_eq!(stream.lines(), canonical(&source, None));
    }
    let work = stream.work();
    eprintln!("reference paragraphs: {work:?}");
    assert!(work.parsed_bytes < source.len() * 8, "{work:?}");
    assert!(work.reference_bytes < source.len() * 12, "{work:?}");
    let pointer = stream.lines()[0].spans[0].content.as_ptr();
    for _ in 0..20 {
        assert_eq!(stream.lines()[0].spans[0].content.as_ptr(), pointer);
    }
    assert_eq!(stream.work(), work);
}

#[test]
fn reference_context_late_definition_replays_once_then_reuses_history() {
    let mut stream = collector();
    let mut source = "See [id].\n\n".repeat(1000);
    stream.push_delta(&source);
    stream.lines();
    let definition = "[id]: https://example.com\n\nAfter.\n\n";
    source.push_str(definition);
    stream.push_delta(definition);
    assert_eq!(stream.lines(), canonical(&source, None));
    let after_replay = stream.work();
    for _ in 0..200 {
        let chunk = "More [id].\n\n";
        source.push_str(chunk);
        stream.push_delta(chunk);
        stream.lines();
    }
    assert_eq!(stream.lines(), canonical(&source, None));
    let parsed = stream.work().parsed_bytes - after_replay.parsed_bytes;
    eprintln!("reference append after late definition: {parsed} parsed bytes");
    assert!(parsed < 200 * 100, "{parsed}");
}

#[test]
fn reference_context_long_document_retains_the_document_expansion_budget() {
    let mut stream = collector();
    let mut source = "[id]: https://example.com\n\n".to_owned();
    stream.push_delta(&source);
    for _ in 0..3000 {
        let chunk = "Read the detailed documentation at [the reference][id].\n\n";
        source.push_str(chunk);
        stream.push_delta(chunk);
        stream.lines();
    }
    assert_eq!(stream.lines(), canonical(&source, None));
    let work = stream.work();
    eprintln!("long reference document: {work:?}");
    assert!(work.parsed_bytes < source.len() * 8, "{work:?}");
    assert!(work.reference_bytes < source.len() * 12, "{work:?}");
}

#[test]
fn reference_context_matches_canonical_at_every_character_boundary() {
    let sources = [
        "See [STRASSE].\n\n[Straße]: https://example.com\n\nNext [strasse].\n",
        "[id]: first \"first title\"\n\n[ID]: second\n\nSee [ID].\n\nMore.\n",
        "See [docs][id].\n\n[id]: <https://example.com/a>\n  \"later title\"\n\nAfter [id].\n",
        "[a b]: target\n\nSee [A\nB].\n\nNext ![image][a b].\n",
        "[id]: first\n\nOne [id].\n\nTwo [new].\n\n[new]: second\n\nThree [id] [new].\n",
        "[id]: first\n\n- See [id].\n- Next [id].\n\n> Quote [id].\n\nAfter.\n",
        "[id]: first\n\n```rust\nlet value = \"[id]\";\n```\n\nSee [id].\n",
        "[id]: <a\\*b> 'a &amp; b'\n\nSee [id] and [missing].\n\nAfter.\n",
        "See [id].\n\n[id]: <target>invalid\n\nAfter [id].\n",
    ];
    for source in sources {
        let boundaries: Vec<_> = source
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(source.len()))
            .collect();
        for &split in &boundaries {
            assert_live_equals_full(&[&source[..split], &source[split..]]);
        }
        let chunks: Vec<_> = boundaries
            .windows(2)
            .map(|pair| &source[pair[0]..pair[1]])
            .collect();
        assert_live_equals_full(&chunks);
    }
}

#[test]
fn reference_context_preserves_expansion_limits_as_source_grows() {
    let mut stream = collector();
    let mut source = format!("[id]: https://example.com/{}\n\n", "x".repeat(2048));
    stream.push_delta(&source);
    stream.lines();
    for _ in 0..60 {
        source.push_str("[id]\n\n");
        stream.push_delta("[id]\n\n");
        assert_eq!(stream.lines(), canonical(&source, None));
    }
    // Growing input increases the canonical parser's expansion budget and can
    // resolve links that were previously left literal.
    let padding = format!("{}\n\n", "plain ".repeat(25_000));
    source.push_str(&padding);
    stream.push_delta(&padding);
    assert_eq!(stream.lines(), canonical(&source, None));
    // A cold full parse has more fuel than its would-be stable prefix. The
    // prefix must not freeze the literal links from a smaller parser budget.
    let mut cold = collector();
    cold.push_delta(&source);
    assert_eq!(cold.lines(), canonical(&source, None));
    cold.push_delta("After.\n\n");
    assert_eq!(
        cold.lines(),
        canonical(&format!("{source}After.\n\n"), None)
    );
    stream.finalize();
    assert_eq!(stream.lines(), canonical(&source, None));
}

#[test]
fn reference_context_tail_preserves_the_full_document_expansion_budget() {
    let mut stream = collector();
    let mut source = format!(
        "[id]: https://example.com/{}\n\n{}\n\nMutable [id].\n",
        "x".repeat(2048),
        "plain ".repeat(35_000),
    );
    stream.push_delta(&source);
    stream.lines();
    assert!(stream.stable_source_len > 200_000);
    for index in 0..60 {
        source.push_str("[id] ");
        stream.push_delta("[id] ");
        assert!(
            stream.lines() == canonical(&source, None),
            "reference expansion differs at append {index}"
        );
    }
}

#[test]
fn reference_context_does_not_publish_definitions_after_a_held_table() {
    let before = "See [id].\n\n";
    let source =
        format!("{before}| A | B |\n| --- | --- |\n| a | b |\n\n[id]: https://example.com\n");
    for (split, _) in source.char_indices().chain([(source.len(), '\0')]) {
        let mut stream = collector();
        stream.push_delta(&source[..split]);
        stream.lines();
        stream.push_delta(&source[split..]);
        assert_eq!(stream.lines(), canonical(before, None), "split {split}");
        stream.finalize();
        assert_eq!(stream.lines(), canonical(&source, None));
    }
}

#[test]
fn reference_context_replacement_resets_definitions_and_expansion_guard() {
    let mut stream = collector();
    stream.push_delta(&format!(
        "[id]: https://example.com/{}\n\n{}",
        "x".repeat(2048),
        "[id]\n\n".repeat(60)
    ));
    stream.lines();
    let fresh = "[id]: first\n\nSee [id].\n\nNext.\n\n";
    stream.replace_source(fresh);
    assert_eq!(stream.lines(), canonical(fresh, None));
    let changed = fresh.replace("first", "other");
    stream.replace_source(&changed);
    assert_eq!(stream.lines(), canonical(&changed, None));
    let before = stream.work();
    for _ in 0..200 {
        stream.push_delta("Next [id].\n\n");
        stream.lines();
    }
    assert!(stream.work().parsed_bytes - before.parsed_bytes < 200 * 100);
}

#[test]
fn quoted_table_holds_its_whole_top_level_block() {
    let mut stream = collector();
    let source = "Intro.\n\n> Quote\n>\n> | A | B |\n> | --- | --- |\n> | long | value |\n";
    for line in source.split_inclusive('\n') {
        stream.push_delta(line);
        stream.lines();
    }
    assert_eq!(stream.lines(), canonical("Intro.\n\n", None));
    stream.finalize();
    assert_eq!(stream.lines(), canonical(source, None));
}

#[test]
fn table_like_code_lines_are_not_held() {
    assert_live_equals_full(&[
        "Before.\n\n",
        "```text\n",
        "| A | B |\n",
        "| --- | --- |\n",
        "```\n",
        "After.\n",
    ]);
}

#[test]
fn source_replacement_resets_all_boundaries() {
    let mut stream = collector();
    stream.push_delta("Old paragraph.\n\n| A | B |\n| --- | --- |\n");
    stream.lines();
    stream.replace_source("Replacement\n\nNew tail");
    assert_eq!(stream.lines(), canonical("Replacement\n\nNew tail", None));
    stream.push_delta(".\n");
    stream.finalize();
    assert_eq!(
        stream.lines(),
        canonical("Replacement\n\nNew tail.\n", None)
    );
}

#[test]
fn structural_streams_match_canonical_at_each_unicode_split() {
    let cases = [
        "# Heading\n\nParagraph **bold** and *italic*.\ncontinued.\n\n---\n\nEnd.",
        "Before.\n\n```rust\n/* comment\ncontinued */\nfn main() {}\n```\n\nAfter.",
        "Before.\n\n- first\n- second\n\n  paragraph\n\n> quote\n> next\n\nEnd.",
        "<div>html</div>\n\nParagraph.\n\n<section>more</section>\n\nEnd.",
        "Unicode \u{4e16}\u{754c} \u{1f642} e\u{301}.\n\n[local](./src/lib.rs:12)\n\nEnd.",
        "See [id].\n\nA separate block.\n\n[id]: https://example.com\n\nEnd.",
    ];
    for source in cases {
        for chunk_size in [1, 3, 7, 19, 1000] {
            let chars: Vec<_> = source.chars().collect();
            let chunks: Vec<String> = chars
                .chunks(chunk_size)
                .map(|chunk| chunk.iter().collect())
                .collect();
            assert_live_equals_full(&chunks.iter().map(String::as_str).collect::<Vec<_>>());
        }
    }
}

#[test]
fn growing_code_fence_has_linear_parse_bytes() {
    for language in ["rust", "unknown-language", ""] {
        let mut stream = collector();
        let mut source = format!("Before.\n\n```{language}\n");
        stream.push_delta(&source);
        stream.lines();
        for _ in 0..200 {
            let chunk = "let value = 1;\n";
            source.push_str(chunk);
            stream.push_delta(chunk);
            stream.lines();
        }
        let work = stream.work();
        eprintln!("open fence ({language:?}): {work:?}");
        assert!(
            work.parsed_bytes <= source.len() * 8,
            "{language}: {work:?}"
        );
        assert!(work.fence_bytes <= source.len() * 8, "{language}: {work:?}");
        assert!(work.rendered_rows <= 200 * 8, "{language}: {work:?}");
        assert_eq!(stream.lines(), canonical(&source, None));
        source.push_str("```\n\nAfter.");
        stream.push_delta("```\n\nAfter.");
        assert_eq!(stream.lines(), canonical(&source, None));
    }
}

#[test]
fn adjacent_top_level_blocks_match_canonical_during_streaming() {
    let blocks = [
        "Paragraph.\n\n",
        "# Heading\n",
        "> Quote\n\n",
        "---\n\n",
        "```rust\n```\n\n",
        "```\ncode\n```\n\n",
        "    indented code\n\n",
        "<div>html</div>\n\n",
        "- first\n\n",
        "[id]: https://example.com\n\n",
    ];
    for first in blocks {
        for second in blocks {
            let source = format!("{first}{second}End.");
            let chars: Vec<_> = source.chars().collect();
            let chunks: Vec<String> = chars
                .chunks(3)
                .map(|chunk| chunk.iter().collect())
                .collect();
            assert_live_equals_full(&chunks.iter().map(String::as_str).collect::<Vec<_>>());
        }
    }
}

#[test]
fn retained_block_offsets_preserve_indentation_after_a_stable_prefix() {
    for chunks in [
        vec!["Before.\n\n", "    indented code\n", "\nEnd."],
        vec![
            "Before.\n\n",
            "  ```rust\n",
            "  let value = 1;\n",
            "  ```\n\nEnd.",
        ],
    ] {
        assert_live_equals_full(&chunks);
    }
}

#[test]
fn fence_highlight_line_limit_recolors_retained_rows_once() {
    let mut stream = collector();
    let line = "let value = 1;\n";
    let mut source = format!("```rust\n{}", line.repeat(10_000));
    stream.push_delta(&source);
    stream.lines();
    stream.push_delta(line);
    source.push_str(line);
    assert_eq!(stream.lines(), canonical(&source, None));
    let work = stream.work();
    stream.push_delta(line);
    source.push_str(line);
    assert_eq!(stream.lines(), canonical(&source, None));
    assert_eq!(stream.work().parses, work.parses);
}

#[test]
fn long_plain_paragraph_parse_work_is_linear() {
    let mut measurements = Vec::new();
    for (name, chunk) in [
        ("inline words", "ordinary words "),
        ("soft breaks", "ordinary paragraph line\n"),
    ] {
        let mut stream = collector();
        let mut source = String::new();
        for _ in 0..200 {
            source.push_str(chunk);
            stream.push_delta(chunk);
            stream.lines();
        }
        assert_eq!(stream.lines(), canonical(&source, None), "{name}");
        let work = stream.work();
        eprintln!("{name}: {} source bytes, {work:?}", source.len());
        measurements.push((name, source.len(), work));
    }
    for (name, bytes, work) in measurements {
        assert!(work.parsed_bytes <= bytes * 8, "{name}: {work:?}");
        assert!(work.plain_bytes <= bytes * 2, "{name}: {work:?}");
        assert!(work.fence_bytes <= bytes * 2, "{name}: {work:?}");
        assert!(work.rendered_rows <= 200 * 2, "{name}: {work:?}");
    }
}

#[test]
fn plain_paragraphs_match_canonical_at_every_split_and_character() {
    for source in [
        "Ordinary words, with punctuation! Isn't this ($42.50) a/b + c-d = e? {yes}\nNext: 123.",
        "Letters \u{4e2d}\u{6587} cafe\u{301} 👩‍💻\n\u{4e2d}\u{6587} next.",
        "Trailing spaces   then text \nNext line ",
        "Hard break  \nNext line",
        "Plain\n\nAnother paragraph\n",
        "Plain\n===\nAfter heading\n",
        "Plain\n---\nAfter heading\n",
        "Plain\n    Indented continuation\n",
        "Plain\n1. Ordered list\n",
        "Plain **bold** and _italic_ with `code`.",
        "Plain [reference]\n\n[reference]: destination\n",
        "Plain &amp; entity and \\*escaped\\* text.",
        "Plain <https://example.com> and <br> HTML.",
        "Plain\r\nCRLF\r\nnext\0end",
        "Plain\twith tab\u{a0}space\u{2028}separator",
        "\u{feff}Plain words\nNext line",
        "Plain words\n\u{feff}Next line",
        "# Heading\n\nPlain words\nNext line\n",
        "[id]: destination\n\nSee [id].\n\nPlain words\nNext line\n",
        "```\ncode\n```\n\nPlain words\nNext line",
    ] {
        for (split, _) in source.char_indices().chain([(source.len(), '\0')]) {
            assert_live_equals_full(&[&source[..split], &source[split..]]);
        }
        let chunks = source
            .char_indices()
            .map(|(start, ch)| &source[start..start + ch.len_utf8()])
            .collect::<Vec<_>>();
        assert_live_equals_full(&chunks);
    }
}

#[test]
fn plain_tail_after_completed_blocks_only_visits_new_source() {
    for prefix in [
        "# Heading\n\n",
        "[id]: destination\n\nSee [id].\n\n",
        "```\ncode\n```\n\n",
    ] {
        let mut stream = collector();
        let mut source = format!("{prefix}Ordinary line\n");
        stream.push_delta(&source);
        stream.lines();
        let before = stream.work();
        for _ in 0..200 {
            let chunk = "Another line with words.\n";
            source.push_str(chunk);
            stream.push_delta(chunk);
            stream.lines();
        }
        let work = stream.work();
        assert_eq!(
            work.parsed_bytes, before.parsed_bytes,
            "{prefix:?}: {work:?}"
        );
        assert_eq!(work.fence_bytes, before.fence_bytes, "{prefix:?}: {work:?}");
        assert!(work.plain_bytes <= source.len() * 2, "{prefix:?}: {work:?}");
        assert_eq!(stream.lines(), canonical(&source, None), "{prefix:?}");
    }
}

#[test]
fn rejected_plain_candidate_is_not_rescanned_without_a_new_block_boundary() {
    let mut stream = collector();
    stream.push_delta("Plain *styled* paragraph\n");
    stream.lines();
    let examined = stream.work().plain_bytes;
    for _ in 0..100 {
        stream.push_delta("More words\n");
        stream.lines();
    }
    assert_eq!(stream.work().plain_bytes, examined);
}

#[test]
fn plain_paragraph_ascii_boundaries_match_canonical() {
    for byte in 0..=127 {
        let ch = char::from(byte).to_string();
        assert_live_equals_full(&["Ordinary words ", &ch, " next\nFinal line"]);
        assert_live_equals_full(&["Ordinary words\n", &ch, " next\nFinal line"]);
    }
}

#[test]
fn plain_paragraph_replacement_resets_pending_spaces_and_reenables_incremental_work() {
    let mut stream = collector();
    stream.push_delta("Old words   ");
    stream.lines();
    let epoch = stream.rendered_stream().epoch;
    stream.replace_source("New words ");
    assert_eq!(stream.lines(), canonical("New words ", None));
    assert_ne!(stream.rendered_stream().epoch, epoch);
    let parsed = stream.work().parsed_bytes;
    stream.push_delta("remain\nAnother line\n");
    assert_eq!(
        stream.lines(),
        canonical("New words remain\nAnother line\n", None)
    );
    assert_eq!(stream.work().parsed_bytes, parsed);
}
