use crate::tui::message_role::MessageRole;
use crate::{
    config::TuiThemeConfig,
    tui::{
        markdown_render::render_markdown_text_with_width_and_cwd,
        state::{RuntimePhase, RuntimeSnapshot, TranscriptEntry, TranscriptTurn},
        testing::TuiHarness,
        theme,
    },
};

const SOURCE: &str = "# Heading\n\n```rust\nlet value = 1;\n";

// Theme installation is process-wide. A child keeps these mutations out of
// concurrently running app fixtures without requiring all tests to share a lock.
fn isolated_theme_test() -> bool {
    let thread = std::thread::current();
    let name = thread.name().expect("named test thread");
    if std::env::var("RARA_THEME_TEST").as_deref() == Ok(name) {
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", name, "--nocapture"])
        .env("RARA_THEME_TEST", name)
        .output()
        .expect("spawn isolated theme test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

fn themes() -> [TuiThemeConfig; 2] {
    let mut semantic = TuiThemeConfig::default();
    semantic
        .tokens
        .insert("markdown.heading".into(), "#123456".into());
    let mut syntax = semantic.clone();
    syntax.syntax_theme = Some(
        two_face::theme::EmbeddedThemeName::InspiredGithub
            .as_name()
            .into(),
    );
    [semantic, syntax]
}

#[test]
fn cached_active_prefix_refreshes_styled_rows_on_theme_changes() {
    if !isolated_theme_test() {
        return;
    }
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    harness.app_mut().push_entry(MessageRole::Agent, SOURCE);
    harness
        .app_mut()
        .set_runtime_phase(crate::tui::state::RuntimePhase::ProcessingResponse, None);
    harness
        .app_mut()
        .append_agent_thinking_delta("```rust\nlet answer = 42;\n");
    harness.app_mut().active_live.thinking_started_at = None;
    let mut rows = super::renderable_transcript_lines(harness.app(), 80);
    for config in themes() {
        theme::install_config(&config);
        let changed = super::renderable_transcript_lines(harness.app(), 80);
        assert_ne!(
            rows.iter().cloned().collect::<Vec<_>>(),
            changed.iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(
            changed.iter().cloned().collect::<Vec<_>>(),
            super::transcript_cache_tests::canonical_rows(harness.app(), 80)
        );
        let count = harness.app().active_assembly_count.get();
        theme::install_config(&config);
        super::renderable_transcript_lines(harness.app(), 80);
        assert_eq!(harness.app().active_assembly_count.get(), count);
        rows = changed;
    }
}

#[test]
fn committed_rows_refresh_after_theme_installation() {
    if !isolated_theme_test() {
        return;
    }
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    harness
        .app_mut()
        .restore_committed_turns(vec![TranscriptTurn {
            thinking_duration: None,
            entries: vec![TranscriptEntry::new(MessageRole::Agent, SOURCE)],
        }]);
    let mut rows = super::renderable_transcript_lines(harness.app(), 80);
    for config in themes() {
        theme::install_config(&config);
        let expected = super::transcript_cache_tests::canonical_rows(harness.app(), 80);
        assert_ne!(rows.iter().cloned().collect::<Vec<_>>(), expected);
        rows = super::renderable_transcript_lines(harness.app(), 80);
        assert_eq!(rows.iter().cloned().collect::<Vec<_>>(), expected);
        theme::install_config(&config);
        let unchanged = super::renderable_transcript_lines(harness.app(), 80);
        assert!(std::ptr::eq(
            rows.get(0).unwrap(),
            unchanged.get(0).unwrap()
        ));
    }
}

#[test]
fn streamed_rows_refresh_without_new_source_after_theme_installation() {
    if !isolated_theme_test() {
        return;
    }
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    harness
        .app_mut()
        .push_entry(MessageRole::User, "Show the themed stream.");
    harness
        .app_mut()
        .set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    harness.app_mut().append_agent_delta(SOURCE);
    let stream = harness.app().agent_markdown_stream.as_ref().unwrap();
    let mut lines = stream.display_lines().to_vec();
    let mut visual_rows = super::renderable_transcript_lines(harness.app(), 80);
    for config in themes() {
        theme::install_config(&config);
        let expected = render_markdown_text_with_width_and_cwd(SOURCE, None, None).lines;
        assert_ne!(lines, expected);
        lines = stream.display_lines().to_vec();
        assert_eq!(lines, expected);
        let before = stream.markdown_work();
        theme::install_config(&config);
        assert_eq!(&*stream.display_lines(), expected);
        assert_eq!(stream.markdown_work(), before);
        let rows = super::renderable_transcript_lines(harness.app(), 80);
        assert_ne!(
            visual_rows.iter().cloned().collect::<Vec<_>>(),
            rows.iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(
            rows.iter().cloned().collect::<Vec<_>>(),
            super::transcript_cache_tests::canonical_rows(harness.app(), 80)
        );
        visual_rows = rows;
    }
}

#[test]
fn plain_paragraph_reenables_incremental_work_after_theme_replay() {
    if !isolated_theme_test() {
        return;
    }
    let mut source = "# Heading\n\nPlain words\n".to_string();
    let cwd = std::env::temp_dir();
    let mut stream = crate::tui::markdown_stream::MarkdownStreamCollector::new(None, &cwd);
    stream.push_delta(&source);
    stream.lines();
    for config in themes() {
        let epoch = stream.rendered_stream().epoch;
        theme::install_config(&config);
        assert_eq!(
            stream.lines(),
            render_markdown_text_with_width_and_cwd(&source, None, Some(&cwd)).lines
        );
        assert_ne!(stream.rendered_stream().epoch, epoch);
        let parsed = stream.work().parsed_bytes;
        let chunk = "Another plain line\n";
        source.push_str(chunk);
        stream.push_delta(chunk);
        assert_eq!(
            stream.lines(),
            render_markdown_text_with_width_and_cwd(&source, None, Some(&cwd)).lines
        );
        assert_eq!(stream.work().parsed_bytes, parsed);
    }
}

#[test]
fn growing_visual_prefix_is_rebuilt_after_theme_replay() {
    if !isolated_theme_test() {
        return;
    }
    let mut source = format!("# Heading\n\n{}", "ordinary words ".repeat(40));
    let cwd = std::env::temp_dir();
    let mut stream = crate::tui::markdown_stream::MarkdownStreamCollector::new(None, &cwd);
    let mut layout = super::StreamRowCache::default();
    stream.push_delta(&source);
    stream.lines();
    let original = layout.materialize(stream.rendered_stream(), 8, super::ResponseView::Full);
    let frozen = original.iter().cloned().collect::<Vec<_>>();
    for config in themes() {
        theme::install_config(&config);
        for chunk in ["", "more words "] {
            source.push_str(chunk);
            stream.push_delta(chunk);
            stream.lines();
            let rows = layout.materialize(stream.rendered_stream(), 8, super::ResponseView::Full);
            let full = render_markdown_text_with_width_and_cwd(&source, None, Some(&cwd)).lines;
            assert_eq!(
                rows.iter().cloned().collect::<Vec<_>>(),
                super::stream_rows_tests::canonical_response(&full, 8, super::ResponseView::Full)
            );
        }
        assert_eq!(original.iter().cloned().collect::<Vec<_>>(), frozen);
    }
}
