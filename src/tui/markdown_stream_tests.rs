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
