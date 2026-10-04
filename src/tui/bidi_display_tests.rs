use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line, widgets::Widget};

use super::composer_text::{WrapConfig, wrapped_text};
use super::display_sanitize::{StreamSanitizer, sanitize_display_text, sanitize_paste_text};
use super::selection::{ScreenPosition, TranscriptSelection};
use super::state::{AgentMarkdownStreamState, Overlay, RuntimeSnapshot};
use super::testing::TuiHarness;
use super::transcript_rows::TranscriptRows;

#[test]
fn bidi_annotations_are_chunk_independent_and_idempotent() {
    for control in [
        '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
        '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ] {
        let source = format!("before{control}after\r\n");
        let expected = format!("before\u{27e6}U+{:04X}\u{27e7}after\n", u32::from(control));
        assert_eq!(sanitize_display_text(&source), expected);
        assert_eq!(sanitize_display_text(&expected), expected);
        assert_eq!(sanitize_paste_text(&source), source.replace("\r\n", "\n"));
        for split in source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain([source.len()])
        {
            let mut sanitizer = StreamSanitizer::default();
            let output =
                sanitizer.push_delta(&source[..split]) + &sanitizer.push_delta(&source[split..]);
            assert_eq!(output, expected, "{control:?} at {split}");
        }
        let hidden = format!("before\u{1b}]hidden{control}\u{7}after");
        assert_eq!(sanitize_display_text(&hidden), "beforeafter");
    }
}

#[test]
fn bidi_policy_preserves_joining_and_emoji_clusters() {
    let source = concat!(
        "a\u{301} a\u{034f}\u{301} \u{628}\u{200c}\u{628} ",
        "\u{1f469}\u{200d}\u{1f4bb} \u{2764}\u{fe0f} ",
        "\u{1f3f4}\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}"
    );
    assert_eq!(sanitize_display_text(source), source);
    assert_eq!(sanitize_paste_text(source), source);
    let rows = TranscriptRows::from_visual_lines(vec![Line::from(source)]);
    assert_eq!(rows.get(0).expect("row").text, source);
}

#[test]
fn bidi_annotations_keep_style_and_selection_matches_buffer() {
    let source = "x\u{202e}y";
    let expected = "x\u{27e6}U+202E\u{27e7}y";
    let style = Style::default().add_modifier(ratatui::style::Modifier::ITALIC);
    let rows = TranscriptRows::from_visual_lines(vec![Line::styled(source, style)]);
    let row = rows.get(0).expect("row");
    assert_eq!(row.text, expected);
    assert_eq!(row.width, 10);
    assert_eq!(row.line.style, style);
    let area = Rect::new(0, 0, 12, 1);
    let mut buffer = Buffer::empty(area);
    row.line.clone().render(area, &mut buffer);
    assert_eq!(buffer[(1, 0)].symbol(), "\u{27e6}");
    assert_eq!(buffer[(9, 0)].symbol(), "y");
    let mut selection = TranscriptSelection::default();
    selection.update_snapshot(&rows, area, 0);
    assert!(selection.start(ScreenPosition::new(0, 0)));
    assert!(selection.drag(ScreenPosition::new(10, 0)));
    assert_eq!(selection.selected_text().as_deref(), Some(expected));
    selection.highlight_visible_range(&mut buffer);
    for x in 0..10 {
        assert!(
            buffer[(x, 0)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
    }
}

#[test]
fn bidi_annotations_survive_markdown_syntax_and_stream_finalization() {
    let source = "**before\u{202e}after**\n\n```text\ncode\u{2066}end\n```\n";
    let expected =
        "**before\u{27e6}U+202E\u{27e7}after**\n\n```text\ncode\u{27e6}U+2066\u{27e7}end\n```\n";
    let mut complete = AgentMarkdownStreamState::new(".".into());
    complete.push_delta(expected);
    for split in source
        .char_indices()
        .map(|(offset, _)| offset)
        .chain([source.len()])
    {
        let mut stream = AgentMarkdownStreamState::new(".".into());
        stream.push_delta(&source[..split]);
        stream.push_delta(&source[split..]);
        assert_eq!(stream.raw_text, expected);
        assert_eq!(*stream.display_lines(), *complete.display_lines());
        stream.finalize_display_lines();
        let mut finalized = AgentMarkdownStreamState::new(".".into());
        finalized.push_delta(expected);
        finalized.finalize_display_lines();
        assert_eq!(*stream.display_lines(), *finalized.display_lines());
    }
}

#[test]
fn bidi_annotation_expansion_keeps_progress_bounded() {
    use super::runtime::apply_tui_event;
    use super::state::{TranscriptEntry, TuiEvent};
    let source = "a\u{202e}b";
    let entry = TranscriptEntry::new(super::message_role::MessageRole::Agent, source);
    assert_eq!(entry.message, "a\u{27e6}U+202E\u{27e7}b");
    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    apply_tui_event(
        tui.app_mut(),
        TuiEvent::ToolProgress {
            call_id: Some("bidi-output".into()),
            name: "bash".into(),
            stream: rara_tools::tool::ToolOutputStream::Stdout,
            chunk: format!("{}END", "\u{202e}".repeat(32 * 1024)),
        },
    );
    let entry = tui.app().active_turn.entries.last().expect("progress");
    assert!(entry.message.len() <= 16 * 1024);
    assert!(entry.message.lines().count() <= 16);
    assert!(entry.message.contains("truncated"));
    assert!(entry.message.ends_with("\u{27e6}U+202E\u{27e7}END\n"));
    assert!(!entry.message.contains('\u{202e}'));
}

#[test]
fn bidi_editor_mapping_retains_original_offsets_across_wrapped_labels() {
    let source = "a\u{202e}b\u{1f469}\u{200d}\u{1f4bb}z";
    let projected = "a\u{27e6}U+202E\u{27e7}b\u{1f469}\u{200d}\u{1f4bb}z";
    for width in [4, 6, 8, 12, 30] {
        let layout = wrapped_text(source, WrapConfig::composer(width));
        let expected = wrapped_text(projected, WrapConfig::composer(width));
        assert_eq!(layout.rows(), expected.rows(), "width {width}");
        for (original, displayed) in [(0, 0), (1, 1), (2, 9), (3, 10), (6, 13), (7, 14)] {
            let actual = layout.position_for_offset(original);
            let expected = expected.position_for_offset(displayed);
            assert_eq!((actual.row, actual.column), (expected.row, expected.column));
            assert_eq!(layout.offset_for_position(actual), original);
        }
        for row in 0..layout.rows().len() {
            for column in 0..usize::from(width) {
                let offset = layout
                    .offset_for_position(super::composer_text::VisualPosition { row, column });
                assert!([0, 1, 2, 3, 6, 7].contains(&offset));
            }
        }
    }
}

#[tokio::test]
async fn bidi_paste_renders_annotation_but_edits_and_submits_original_source() {
    use super::runtime_port::RuntimeCommand;
    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    let source = "a\u{202e}b";
    super::terminal_ui::handle_paste(source.into(), tui.app_mut());
    assert_eq!(tui.app().bottom_pane.input, source);
    tui.app_mut().move_active_input_cursor_left();
    let (screen, cursor) = tui.screen_with_cursor(80, 24);
    assert!(screen.contains("a\u{27e6}U+202E\u{27e7}b"), "{screen}");
    let (x, y) = cursor.expect("composer cursor");
    assert_eq!(x, 11);
    assert_eq!(
        screen
            .lines()
            .nth(y as usize)
            .unwrap()
            .chars()
            .nth(x as usize),
        Some('b')
    );
    tui.press_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(tui.app().bottom_pane.input, "ab");
    tui.press_key(KeyEvent::new(KeyCode::Char('\u{202e}'), KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(tui.app().bottom_pane.input, source);
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    tui.expect_command(RuntimeCommand::Input(
        crate::runtime_control::InputControlRequest::SubmitUserPrompt {
            prompt: source.into(),
        },
    ));
}

#[test]
fn bidi_setup_and_search_editors_keep_cursor_after_annotation() {
    for overlay in [
        Overlay::BaseUrlEditor,
        Overlay::ModelNameEditor,
        Overlay::OpenAiProfileLabelEditor,
        Overlay::ModelSearch,
    ] {
        let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
        tui.app_mut().open_overlay(overlay);
        super::terminal_ui::handle_paste("a\u{202e}b".into(), tui.app_mut());
        tui.app_mut().move_active_input_cursor_left();
        let (buffer, cursor) = tui.screen_buffer(100, 30);
        let (x, y) = cursor.expect("editor cursor");
        assert_eq!(buffer[(x, y)].symbol(), "b");
        assert_eq!(buffer[(x - 1, y)].symbol(), "\u{27e7}");
        assert_eq!(buffer[(x - 8, y)].symbol(), "\u{27e6}");
    }
}

#[test]
fn bidi_resume_search_is_visible_and_credentials_remain_masked() {
    use super::state::{ApiKeyTarget, ListPickerKind};
    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    tui.app_mut()
        .open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
    super::terminal_ui::handle_paste("a\u{202e}b".into(), tui.app_mut());
    assert_eq!(tui.app().resume_search_query, "a\u{202e}b");
    assert!(
        tui.screen_text(100, 30)
            .contains("a\u{27e6}U+202E\u{27e7}b")
    );

    tui.app_mut()
        .open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible));
    super::terminal_ui::handle_paste("a\u{202e}b".into(), tui.app_mut());
    tui.app_mut().move_active_input_cursor_left();
    let (screen, _) = tui.screen_with_cursor(100, 30);
    assert!(screen.contains("***"));
    assert!(!screen.contains("U+202E"));
    assert_eq!(tui.app().api_key_input, "a\u{202e}b");
    let (buffer, cursor) = tui.screen_buffer(100, 30);
    let (x, y) = cursor.expect("masked cursor");
    assert_eq!(buffer[(x, y)].symbol(), "*");
    assert_eq!(buffer[(x - 2, y)].symbol(), "*");
}
