use super::*;

fn assert_chunks_match_canonical(chunks: &[&str]) {
    let cwd = PathBuf::from(".");
    let mut stream = AgentMarkdownStreamState::new(cwd.clone());
    for chunk in chunks {
        stream.push_delta(chunk);
        let expected = scrub_internal_control_tokens(&stream.raw_text);
        assert_eq!(
            stream.last_visible_text, expected,
            "source: {:?}, chunk: {chunk:?}",
            stream.raw_text
        );
        let mut collector = MarkdownStreamCollector::new(None, &cwd);
        collector.push_delta(&expected);
        collector.lines();
        assert_eq!(&*stream.display_lines(), collector.cached_lines());
    }
    stream.finalize_display_lines();
    let mut collector = MarkdownStreamCollector::new(None, &cwd);
    collector.push_delta(&scrub_internal_control_tokens(&stream.raw_text));
    collector.finalize();
    assert_eq!(&*stream.display_lines(), collector.cached_lines());
}

#[test]
fn legacy_marker_terminator_can_arrive_in_the_next_chunk() {
    assert_chunks_match_canonical(&["Before<channel|", ">After"]);
}

#[test]
fn internal_block_separator_can_arrive_after_the_close_tag() {
    assert_chunks_match_canonical(&["Before<agent_runtime>hidden</agent_runtime>", "After"]);
}

#[test]
fn late_dsml_evidence_revises_literal_leading_think() {
    assert_chunks_match_canonical(&["<think>private</think>Visible", "|DSML|"]);
}

#[test]
fn orphaned_dsml_tail_stays_hidden_across_later_plain_chunks() {
    assert_chunks_match_canonical(&[
        "Visible\n<|DSML|parameter name=\"path\">file</|DSML|parameter>\n</|DSML|invoke>",
        "must remain hidden",
    ]);
}

#[test]
fn ordinary_angle_brackets_do_not_revisit_accumulated_source() {
    let mut stream = AgentMarkdownStreamState::new(PathBuf::from("."));
    stream.push_delta(&"Stable content.\n\n".repeat(1000));
    for _ in 0..1000 {
        for chunk in ["Vec", "<", "T", ">", " x <= y < 10 <em>text</em>\n"] {
            stream.push_delta(chunk);
        }
    }
    assert_eq!(stream.last_visible_text, stream.raw_text);
    assert_eq!(stream.control_scrubbed_bytes, 0);
    assert_eq!(stream.control_replay.scanned_bytes, stream.raw_text.len());
}

#[test]
fn control_cleanup_matches_canonical_at_every_character_boundary() {
    let fixtures = [
        "Visible:<channel|>After",
        "Before<agent_runtime>hidden</agent_runtime>After",
        "Before<agent_runtime_error>hidden</agent_runtime_error> After",
        "Before<rara_internal_history_context>hidden</rara_internal_history_context>After",
        "Visible:<agent_runtime>unfinished",
        "<think>literal thoughts</think>Visible",
        "<think>literal unfinished thoughts",
        " \n<think>private</think>Visible｜DSML｜",
        "<think>private</think>Visible|DSML|",
        "<think>private<｜end▁of▁sentence｜>",
        "Before<｜end▁of▁sentence｜>After",
        "<chan<｜end▁of▁sentence｜>nel|>After",
        "Before<agent_runtime>hidden</agent_runtime><｜end▁of▁sentence｜>After",
        "Visible\n<|DSML|parameter name=\"path\">file</|DSML|parameter>\n</|DSML|invoke>Later",
        "Visible\n<｜DSML｜parameter name=\"path\">file</｜DSML｜parameter>\n</｜DSML｜invoke>Later",
        "Before\n<|DSML|tool_calls><|DSML|invoke name=\"read_file\"><|DSML|parameter name=\"path\" string=\"true\">Cargo.toml</|DSML|parameter></|DSML|invoke></|DSML|tool_calls>After",
        "Before\n<｜DSML｜tool_calls><｜DSML｜invoke name=\"read_file\"><｜DSML｜parameter name=\"path\" string=\"true\">Cargo.toml</｜DSML｜parameter></｜DSML｜invoke></｜DSML｜tool_calls>After",
        "Before<|DSML|tool_calls>malformed</|DSML|tool_calls>After",
        "Document `path</|DSML|parameter>` as literal markup.",
        "Before<agent_runtime><agent_runtime>nested</agent_runtime>After</agent_runtime>Tail",
        "Before<agent_runtime_error><agent_runtime>nested</agent_runtime_error>After</agent_runtime>Tail",
        "Vec<T> x <= y < 10 <em>text</em> <|> <bad name|> <bad!|> <naïve|>",
        "Visible:<abc_DEF-123|>After<second|>Tail",
        "<<nested|> <<agent_runtime>hidden</agent_runtime>Visible",
        "\u{1b}[31mBefore<agent_\u{1b}[0mruntime>hidden</agent_runtime>After\r\n",
    ];
    for source in fixtures {
        let boundaries: Vec<_> = source
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(source.len()))
            .collect();
        for &split in &boundaries {
            assert_chunks_match_canonical(&[&source[..split], &source[split..]]);
        }
        let chunks: Vec<_> = boundaries
            .windows(2)
            .map(|pair| &source[pair[0]..pair[1]])
            .collect();
        assert_chunks_match_canonical(&chunks);
    }
}

#[test]
fn long_legacy_names_are_incremental_until_the_marker_completes() {
    let mut stream = AgentMarkdownStreamState::new(PathBuf::from("."));
    stream.push_delta("Visible:<");
    for _ in 0..10_000 {
        stream.push_delta("name_1");
    }
    stream.push_delta("|");
    assert_eq!(stream.last_visible_text, stream.raw_text);
    assert_eq!(stream.control_scrubbed_bytes, 0);
    stream.push_delta(">");
    assert_eq!(stream.last_visible_text, "Visible:\n");
    let replayed_bytes = stream.raw_text.len();
    assert_eq!(stream.control_scrubbed_bytes, replayed_bytes);
    for _ in 0..1000 {
        stream.push_delta("Tail ");
    }
    assert_eq!(stream.control_scrubbed_bytes, replayed_bytes);
    assert_eq!(stream.control_replay.scanned_bytes, stream.raw_text.len());
    assert_eq!(stream.last_visible_text, stream.sanitized_raw_text());
}

#[test]
fn empty_sanitized_chunks_do_not_replay_a_control_context() {
    let mut stream = AgentMarkdownStreamState::new(PathBuf::from("."));
    stream.push_delta("Before<agent_runtime>hidden</agent_runtime>");
    let replayed_bytes = stream.control_scrubbed_bytes;
    for chunk in ["", "\u{1b}", "[31m", "\u{1b}[0m"] {
        stream.push_delta(chunk);
    }
    assert_eq!(stream.control_scrubbed_bytes, replayed_bytes);
    assert_eq!(stream.last_visible_text, "Before");
    stream.push_delta("After");
    assert_eq!(stream.last_visible_text, "Before\nAfter");
}
