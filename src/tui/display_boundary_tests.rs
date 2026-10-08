use rara_tools::tool::ToolOutputStream;

use super::runtime::apply_tui_event;
use super::state::{AgentMarkdownStreamState, RuntimeSnapshot, TuiEvent};
use super::testing::TuiHarness;
use crate::runtime_control::{RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, ToolEvent};
use crate::tui::message_role::MessageRole;
use crate::tui::state::NoticeLevel;

#[test]
fn split_ansi_is_removed_before_stream_storage_and_rendering() {
    let mut stream = AgentMarkdownStreamState::new(".".into());
    stream.push_delta("hello \u{1b}[3");
    stream.push_delta("1mred\u{1b}[0");
    stream.push_delta("m!");
    assert_eq!(stream.raw_text, "hello red!");
    assert_eq!(stream.sanitized_raw_text(), "hello red!");
    let rendered = stream
        .display_lines()
        .iter()
        .map(ToString::to_string)
        .collect::<String>();
    assert!(rendered.contains("hello red!"), "{rendered:?}");
}

#[test]
fn split_crlf_is_one_newline_before_markdown_ingestion() {
    let mut stream = AgentMarkdownStreamState::new(".".into());
    stream.push_delta("```text\na\r");
    stream.push_delta("\nb\n```\n");
    assert_eq!(stream.raw_text, "```text\na\nb\n```\n");
    let complete = super::display_sanitize::sanitize_display_text("```text\na\r\nb\n```\n");
    let mut expected = AgentMarkdownStreamState::new(".".into());
    expected.push_delta(&complete);
    assert_eq!(*stream.display_lines(), *expected.display_lines());
}

#[test]
fn ten_megabyte_progress_is_bounded_without_newlines() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    apply_tui_event(
        harness.app_mut(),
        TuiEvent::ToolProgress {
            call_id: None,
            name: "bash".into(),
            stream: ToolOutputStream::Stdout,
            chunk: format!("{}END", "x".repeat(10 * 1024 * 1024)),
        },
    );
    let entry = harness
        .app()
        .active_turn
        .entries
        .last()
        .expect("progress entry");
    assert!(
        entry.message.len() <= 16 * 1024,
        "{} bytes",
        entry.message.len()
    );
    assert!(entry.message.contains("truncated"));
    assert!(entry.message.ends_with("END\n"));
}

fn progress(call_id: &str, chunk: &str) -> TuiEvent {
    TuiEvent::Runtime(Box::new(RuntimeControlEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        provenance: RuntimeProvenance::local_tui("session"),
        turn_id: None,
        sequence: 0,
        event: RuntimeEvent::Tool(ToolEvent::Progress {
            call_id: Some(call_id.into()),
            name: "bash".into(),
            stream: crate::runtime_control::ToolStream::Stdout,
            chunk: chunk.into(),
        }),
    }))
}

#[test]
fn concurrent_same_name_calls_keep_separate_progress() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    for (id, text) in [("a", "FIRST"), ("b", "SECOND"), ("a", "-TAIL")] {
        apply_tui_event(harness.app_mut(), progress(id, text));
    }
    let entries = &harness.app().active_turn.entries;
    assert_eq!(entries.len(), 2);
    assert!(entries[0].message.contains("FIRST-TAIL"));
    assert!(!entries[0].message.contains("SECOND"));
    assert!(entries[1].message.contains("SECOND"));
    assert!(!entries[1].message.contains("FIRST"));
}

#[test]
fn paste_sanitizes_controls_before_composer_storage() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    super::terminal_ui::handle_paste(
        "ok\u{1b}[31mred\u{1b}[0m\u{8}\u{7}\tend".into(),
        harness.app_mut(),
    );
    assert_eq!(harness.app().bottom_pane.input, "okred\tend");
}

#[test]
fn paste_burst_and_editable_overlays_receive_only_sanitized_text() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    let text = format!("\u{1b}]0;title\u{7}{}\r\n\tEND\u{1b}[31m", "x".repeat(1200));
    let expected = format!("{}\n\tEND", "x".repeat(1200));
    super::terminal_ui::handle_paste(text.clone(), harness.app_mut());
    harness.app_mut().flush_composer_paste();
    assert_eq!(
        harness.app().bottom_pane.large_paste_pending[0].content,
        expected
    );
    harness.app_mut().bottom_pane.expand_large_paste();
    assert_eq!(harness.app().bottom_pane.input, expected);
    harness
        .app_mut()
        .open_overlay(super::state::Overlay::ModelSearch);
    super::terminal_ui::handle_paste(text, harness.app_mut());
    assert_eq!(
        harness.app().model_search_query,
        expected.replace('\n', " ")
    );
    assert_eq!(harness.app().bottom_pane.input, expected);
}

#[test]
fn complete_transcript_constructors_remove_terminal_controls() {
    use super::state::{SystemMessageKind, ToolTranscriptStatus, TranscriptEntry};
    let text = "before\u{1b}]secret\nmore\u{1b}\\after\r\n\tEND\u{8}";
    let entries = [
        TranscriptEntry::new(MessageRole::Agent, text),
        TranscriptEntry::tool(Some("call"), "bash", ToolTranscriptStatus::Completed, text),
        TranscriptEntry::system(text, SystemMessageKind::Other),
        TranscriptEntry::compaction(1, 2, 1, text, vec![text.into()]),
    ];
    for entry in entries {
        assert_eq!(entry.message, "before\nmoreafter\n    END");
    }
}

#[test]
fn tool_results_sanitize_mcp_file_and_diff_content() {
    let untrusted = "before\u{1b}]hidden\u{7}SAFE\u{1b}[31mRED\u{1b}[0m\u{8}";
    let patch = serde_json::json!({
        "status": "success",
        "files_changed": 1,
        "updated_files": ["example.rs"],
        "diff_preview": format!("+{untrusted}"),
    })
    .to_string();
    for (name, content) in [
        ("mcp__example__fetch", untrusted),
        ("read_file", untrusted),
        ("apply_patch", patch.as_str()),
    ] {
        let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
        if name == "read_file" {
            // File bodies stay in the runtime; the transcript displays the read action.
            apply_tui_event(
                harness.app_mut(),
                TuiEvent::Runtime(Box::new(RuntimeControlEvent {
                    event_id: "file-use".into(),
                    provenance: RuntimeProvenance::local_tui("session"),
                    turn_id: None,
                    sequence: 0,
                    event: RuntimeEvent::Tool(ToolEvent::Use {
                        call_id: Some("call".into()),
                        name: name.into(),
                        input: serde_json::json!({"path": untrusted}),
                    }),
                })),
            );
        }
        apply_tui_event(
            harness.app_mut(),
            TuiEvent::Runtime(Box::new(RuntimeControlEvent {
                event_id: "tool-result".into(),
                provenance: RuntimeProvenance::local_tui("session"),
                turn_id: None,
                sequence: 1,
                event: RuntimeEvent::Tool(ToolEvent::Result {
                    call_id: Some("call".into()),
                    name: name.into(),
                    content: content.into(),
                    is_error: false,
                }),
            })),
        );
        let entry = harness.app().active_turn.entries.last().unwrap();
        assert!(
            entry.message.contains("SAFERED"),
            "{name}: {:?}",
            entry.message
        );
        assert!(
            entry
                .message
                .chars()
                .all(|ch| !ch.is_control() || ch == '\n'),
            "{name}: {:?}",
            entry.message
        );
        assert!(
            !entry.message.contains("hidden"),
            "{name}: {:?}",
            entry.message
        );
        let screen = harness.screen_text(100, 30);
        assert!(screen.contains("SAFERED"), "{name}: {screen}");
        for forbidden in ["hidden", "[31m", "[0m"] {
            assert!(!screen.contains(forbidden), "{name}: {screen}");
        }
    }
}

#[test]
fn structured_lsp_diagnostics_sanitize_decoded_fields_before_display() {
    let content = serde_json::json!({
        "file": "f.rs",
        "diagnostics": [{
            "file": "f.rs",
            "line": 0,
            "column": 0,
            "severity": "error",
            "message": "\u{1b}]hidden\u{7}SAFE\u{1b}[31mRED\u{1b}[0m\u{8}",
        }],
    })
    .to_string();
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    apply_tui_event(
        harness.app_mut(),
        TuiEvent::Runtime(Box::new(RuntimeControlEvent {
            event_id: "diagnostics".into(),
            provenance: RuntimeProvenance::local_tui("session"),
            turn_id: None,
            sequence: 1,
            event: RuntimeEvent::Tool(ToolEvent::Result {
                call_id: Some("call".into()),
                name: "lsp_diagnostics".into(),
                content,
                is_error: false,
            }),
        })),
    );
    let entry = harness.app().active_turn.entries.last().unwrap();
    // JSON escapes are data in storage; decoded fields need the render boundary too.
    assert!(
        entry
            .message
            .chars()
            .all(|ch| !ch.is_control() || ch == '\n')
    );
    let screen = harness.screen_text(120, 30);
    assert!(screen.contains("SAFERED"), "{screen}");
    for forbidden in ["hidden", "[31m", "[0m"] {
        assert!(!screen.contains(forbidden), "{screen}");
    }
}

#[test]
fn production_event_bus_preserves_tool_progress_identity() {
    let bus = crate::runtime_event_bus::RuntimeEventBus::new(16);
    let mut receiver = bus.subscribe_control();
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    for (id, text) in [("a", "FIRST"), ("b", "SECOND"), ("a", "-TAIL")] {
        bus.send_with_provenance(
            crate::agent::AgentEvent::ToolProgress {
                call_id: id.into(),
                name: "bash".into(),
                stream: ToolOutputStream::Stdout,
                chunk: text.into(),
            },
            RuntimeProvenance::local_tui("session"),
        );
        apply_tui_event(
            harness.app_mut(),
            TuiEvent::Runtime(Box::new(receiver.try_recv().expect("projected progress"))),
        );
    }
    assert_eq!(harness.app().active_turn.entries.len(), 2);
    assert_eq!(
        harness.app().active_turn.entries[0].message,
        "bash stdout:\nFIRST-TAIL\n"
    );
}

#[test]
fn direct_terminal_ids_keep_separate_state_and_end_retires_it() {
    use super::terminal_event::{
        TerminalCommandEvent, TerminalEvent, TerminalOutputDeltaEvent, TerminalStream,
        TerminalTarget,
    };
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    for (id, chunk) in [
        ("a", "first\u{1b}[3"),
        ("b", "other"),
        ("a", "1m-tail\u{1b}]"),
    ] {
        apply_tui_event(
            harness.app_mut(),
            TuiEvent::Terminal(TerminalEvent::OutputDelta(TerminalOutputDeltaEvent {
                target: TerminalTarget::Pty,
                id: Some(id.into()),
                stream: TerminalStream::Stdout,
                chunk: chunk.into(),
            })),
        );
    }
    assert_eq!(
        harness.app().active_turn.entries[0].message,
        "pty stdout:\nfirst-tail\n"
    );
    assert_eq!(
        harness.app().active_turn.entries[1].message,
        "pty stdout:\nother\n"
    );
    apply_tui_event(
        harness.app_mut(),
        TuiEvent::Terminal(TerminalEvent::End(TerminalCommandEvent {
            target: TerminalTarget::Pty,
            id: Some("a".into()),
            status: "completed".into(),
            command: None,
            exit_code: Some(0),
            output: vec![],
            output_path: None,
            is_error: false,
        })),
    );
    apply_tui_event(
        harness.app_mut(),
        TuiEvent::Terminal(TerminalEvent::OutputDelta(TerminalOutputDeltaEvent {
            target: TerminalTarget::Pty,
            id: Some("a".into()),
            stream: TerminalStream::Stdout,
            chunk: "fresh".into(),
        })),
    );
    assert_eq!(
        harness.app().active_turn.entries.last().unwrap().message,
        "pty stdout:\nfresh\n"
    );
}

#[test]
fn assistant_and_thinking_finalization_share_chunk_independent_text() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    for delta in ["thought\u{1b}]hidden\n", "more\u{1b}", "\\\r", "\nend"] {
        harness.app_mut().append_agent_thinking_delta(delta);
    }
    for delta in [
        "answer\u{1b}[3",
        "1m <agent_",
        "runtime>hidden</agent_runtime>visible\r",
        "\nend",
    ] {
        harness.app_mut().append_agent_delta(delta);
    }
    harness.app_mut().finalize_agent_stream(None);
    let entries = &harness.app().active_turn.entries;
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.role.as_str(), entry.message.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Thinking", "thought\nmore\nend"),
            ("Agent", "answer visible\nend"),
        ]
    );
    harness.app_mut().append_agent_delta("fresh\u{1b}]");
    harness.app_mut().finalize_agent_stream(None);
    harness.app_mut().append_agent_delta("new");
    harness.app_mut().finalize_agent_stream(None);
    assert_eq!(
        harness.app().active_turn.entries.last().unwrap().message,
        "new"
    );
}

#[test]
fn terminal_metadata_and_output_render_without_escape_payloads() {
    use super::terminal_event::{TerminalCommandEvent, TerminalEvent, TerminalTarget};
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    apply_tui_event(
        harness.app_mut(),
        TuiEvent::Terminal(TerminalEvent::End(TerminalCommandEvent {
            target: TerminalTarget::Pty,
            id: Some("pty\u{7}".into()),
            status: "completed".into(),
            command: Some("echo \u{1b}[31mSAFE\u{1b}[0m".into()),
            exit_code: Some(0),
            output: vec!["before\u{1b}]secret\nmore\u{1b}\\after".into()],
            output_path: None,
            is_error: false,
        })),
    );
    let screen = harness.screen_text(80, 30);
    assert!(screen.contains("echo SAFE"), "{screen}");
    assert!(screen.contains("before"), "{screen}");
    assert!(screen.contains("moreafter"), "{screen}");
    for forbidden in ["secret", "[31m", "[0m"] {
        assert!(!screen.contains(forbidden), "{screen}");
    }
}

#[test]
fn bottom_pane_status_sanitizes_tool_error_notice_text() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    harness.app_mut().push_notice(
        NoticeLevel::Warning,
        "Warning: \u{1b}]hidden\u{7}\u{1b}[31mFAIL\u{1b}[0m\u{8}",
    );
    let screen = harness.screen_text(80, 30);
    assert!(screen.contains("Warning: FAIL"), "{screen}");
    for forbidden in ["hidden", "[31m", "[0m"] {
        assert!(!screen.contains(forbidden), "{screen}");
    }
}
