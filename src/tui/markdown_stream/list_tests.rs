#![expect(
    clippy::print_stderr,
    reason = "Expose source and row work for growing Markdown lists."
)]

use super::MarkdownStreamCollector;
use crate::tui::markdown_render::render_markdown_text_with_width_and_cwd;

#[test]
fn completed_list_items_have_linear_source_and_row_work() {
    let cwd = std::env::temp_dir();
    let mut measurements = Vec::new();
    for (prefix, marker, body) in [
        ("", "- ", "Ordinary item words"),
        ("Intro\n\n", "1. ", "**Bold** and `code`"),
        ("", "42) ", "A [local](src/lib.rs) and more"),
        ("[id]: https://example.com\n\n", "* ", "See [id] and more"),
        ("", "- ", "Ordinary\n  > Quoted"),
        ("", "- ", "First\n\n  Second paragraph"),
        ("", "- ", "```rust\n  let n = 1;\n  ```"),
        ("", "- ", "```\n  code\n  ```\n"),
        ("", "- ", "[ ]"),
        ("", "- ", "![alt](image.png) and more"),
    ] {
        let mut stream = MarkdownStreamCollector::new(None, &cwd);
        let mut source = prefix.to_string();
        stream.push_delta(prefix);
        stream.lines();
        for _ in 0..200 {
            let chunk = format!("{marker}{body}\n");
            source.push_str(&chunk);
            stream.push_delta(&chunk);
            stream.lines();
        }
        assert_eq!(
            stream.lines(),
            render_markdown_text_with_width_and_cwd(&source, None, Some(&cwd)).lines,
            "{prefix:?}, {marker:?}"
        );
        let work = stream.work();
        eprintln!(
            "{prefix:?}, {marker:?}: {} source bytes, {work:?}",
            source.len()
        );
        measurements.push((source.len(), stream.lines().len(), work));
    }
    for (bytes, rows, work) in measurements {
        assert!(work.parsed_bytes <= bytes * 8, "{work:?}");
        assert!(work.reference_bytes <= bytes * 8, "{work:?}");
        assert!(work.fence_bytes <= bytes * 8, "{work:?}");
        assert!(work.list_seed_bytes <= bytes * 8, "{work:?}");
        // Each update renders two items plus their pending boundary row.
        assert!(work.rendered_rows <= rows * 3, "{work:?}");
    }
}

#[test]
fn list_continuations_match_canonical_at_every_character_and_split() {
    for source in [
        "- One\n- Two\n- Three\n",
        "  - One\n  - Two\n  - Three\n",
        "9. One\n1. **Two**\n1. `Three`\n",
        "0) One\n1) Two\n1) Three\n",
        "- One\n- Two\n  continued 👩‍💻 cafe\u{301}\n- Three\n",
        "- One\n- Two\n\n  paragraph\n- Three\n",
        "- One\n\n- Two\n- Three\n",
        "- One\n  > Quoted\n- Two\n\n- Three\n",
        "- One\n  - Nested\n- Two\n\n- Three\n",
        "- > Quoted\n\n- ```\n  code\n  ```\n- Third\n  > More\n",
        "- ```\n  code\n  ```\n\n- ```\n  next\n  ```\n- Third\n  > More\n",
        "- ```\n  code\n  ```\n- Two\n\n  paragraph\n- Three\n",
        "Intro\n\n- One\n- Two\n\nAfter\n- Three\n",
        "- [local](src/lib.rs)\n  :12 next\n- Another\n- Third\n",
        "[id]: https://example.com\n\n- [id]\n- [id]\n- [id]\n",
        "- [link][id]\n- Two\n- [id]: path\n- Three\n",
        "- [link][id]\n- Two\n\n[id]: path\nAfter\n",
        "- One\n- Two\n---\nAfter\n",
        "- One\n- Two\n* **\nAfter\n",
        "- One\n- Two\n+ Three\n",
        "- One\r\n- Two\r\n- Three\r\n",
        "- One\n- Two\n- \n\nAfter\n",
        "- One\n\n- \n- Third\n",
        "- \n\n- Words\n  > Quote\n- Last\n",
        "- \n\n- \n\n- Words\n  > Quote\n- Last\n",
        "1. \n\n1. ```\n   code\n   ```\n1. Words\n   > Quote\n",
        "- \n\n- > Quote\n\n  > Next\n- Words\n  > Quote\n",
        "1. First\n2. Next\n3. Third\n   continued\n",
        "999999999. First\n1. Second\n1. Third\n",
        "- [ ] First\n- [x] Second\n- [ ]\n- [X] Last\n",
        "- [ ] First\n\n- [x] Second\n\n  paragraph\n- Last\n",
        "- ![alt](image.png)\n- [![alt](image.png)](src/lib.rs)\n- ![](image.png)\n",
    ] {
        let mut stream = MarkdownStreamCollector::new(None, &std::env::temp_dir());
        for (offset, ch) in source.char_indices() {
            let end = offset + ch.len_utf8();
            stream.push_delta(&source[offset..end]);
            assert_canonical(&mut stream, &source[..end]);
        }
        stream.finalize();
        assert_canonical(&mut stream, source);
        for (split, _) in source.char_indices() {
            let mut stream = MarkdownStreamCollector::new(None, &std::env::temp_dir());
            stream.push_delta(&source[..split]);
            assert_canonical(&mut stream, &source[..split]);
            stream.push_delta(&source[split..]);
            assert_canonical(&mut stream, source);
        }
    }
}

fn assert_canonical(stream: &mut MarkdownStreamCollector, source: &str) {
    assert_eq!(
        stream.lines(),
        render_markdown_text_with_width_and_cwd(source, None, Some(&std::env::temp_dir())).lines,
        "source {source:?}"
    );
}

#[test]
fn nested_table_rewinds_retained_list_rows_before_holdback() {
    let mut stream = MarkdownStreamCollector::new(None, &std::env::temp_dir());
    let first = "- One\n- Two\n";
    stream.push_delta(first);
    stream.lines();
    let epoch = stream.rendered_stream().epoch;
    stream.push_delta("  | a | b |\n  | - | - |\n");
    assert!(stream.lines().is_empty());
    assert_ne!(stream.rendered_stream().epoch, epoch);
    stream.finalize();
    assert_canonical(&mut stream, &format!("{first}  | a | b |\n  | - | - |\n"));
}

#[test]
fn replacing_a_list_discards_numbering_and_loose_context() {
    let mut stream = MarkdownStreamCollector::new(None, &std::env::temp_dir());
    stream.push_delta("42. First\n\n1. Second\n");
    stream.lines();
    let epoch = stream.rendered_stream().epoch;
    stream.replace_source("- New\n- Next\n");
    assert_canonical(&mut stream, "- New\n- Next\n");
    assert_ne!(stream.rendered_stream().epoch, epoch);
    stream.push_delta("- Last\n");
    assert_canonical(&mut stream, "- New\n- Next\n- Last\n");
}

#[test]
fn list_item_block_combinations_keep_canonical_spacing() {
    let bodies = [
        "Words\n  > Quote",
        "> Quote\n\n  > Next",
        "```\n  Code\n\n  More\n  ```",
        "<div>\n  Content\n  </div>",
        "<!--\n  Comment\n  -->",
        "    Indented code",
        "- Nested\n\n  - Next",
        "[id]: path",
        "[ ]",
        "![alt](image.png)",
        "",
    ];
    for first in bodies {
        for last in bodies {
            let source = format!("- {first}\n- Marker\n- {last}\n- Done\n");
            let mut stream = MarkdownStreamCollector::new(None, &std::env::temp_dir());
            for (offset, ch) in source.char_indices() {
                let end = offset + ch.len_utf8();
                stream.push_delta(&source[offset..end]);
                assert_canonical(&mut stream, &source[..end]);
            }
        }
    }
}
