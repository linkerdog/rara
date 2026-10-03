use super::*;
use crate::tui::state::RuntimeSnapshot;
use crate::tui::testing::TuiHarness;

fn source(id: &str, stream: ToolOutputStream) -> ProgressSource {
    ProgressSource {
        call_id: Some(id.into()),
        name: "bash".into(),
        stream,
    }
}

#[test]
fn invisible_escape_prefix_keeps_state_without_a_card() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    let input = source("a", ToolOutputStream::Stdout);
    assert!(!append_tool_progress(
        harness.app_mut(),
        input.clone(),
        "\u{1b}]secret"
    ));
    assert!(harness.app().active_turn.entries.is_empty());
    assert!(append_tool_progress(
        harness.app_mut(),
        input.clone(),
        "\u{1b}\\hello\r"
    ));
    assert!(append_tool_progress(harness.app_mut(), input, "\nworld"));
    assert_eq!(
        harness.app().active_turn.entries[0].message,
        "bash stdout:\nhello\nworld\n"
    );
}

#[test]
fn interleaved_streams_do_not_share_escape_or_cr_state() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    let stdout = source("a", ToolOutputStream::Stdout);
    let stderr = source("a", ToolOutputStream::Stderr);
    let other = source("b", ToolOutputStream::Stdout);
    append_tool_progress(harness.app_mut(), stdout.clone(), "first\u{1b}[3");
    append_tool_progress(harness.app_mut(), stderr, "error\r\n");
    append_tool_progress(harness.app_mut(), other, "other");
    append_tool_progress(harness.app_mut(), stdout, "1m-tail");
    let entries = &harness.app().active_turn.entries;
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        [
            "bash stdout:\nfirst-tail\n",
            "bash stderr:\nerror\n",
            "bash stdout:\nother\n",
        ]
    );
}

#[test]
fn byte_and_line_bounds_hold_after_every_multibyte_delta() {
    let mut buffer = ProgressBuffer::new(source("a", ToolOutputStream::Stdout));
    for chunk in [
        "\u{754c}".repeat(64 * 1024),
        "line\n".repeat(1000),
        "10%\r20%\r30%".into(),
        "\tend".into(),
    ] {
        let message = buffer.push_delta(&chunk).expect("visible output");
        assert!(message.len() <= BYTE_LIMIT, "{} bytes", message.len());
        assert!(
            message.lines().count() <= LINE_LIMIT,
            "{} lines",
            message.lines().count()
        );
        let (bytes, capacity) = buffer.tail.retained_size();
        assert!(bytes < BYTE_LIMIT);
        assert!(capacity <= BYTE_LIMIT * 2);
        assert!(message.chars().all(|ch| !ch.is_control() || ch == '\n'));
        assert!(message.contains(TRUNCATION.trim_end()));
    }
}

#[test]
fn long_labels_and_utf8_eviction_fit_the_message_budget() {
    let mut buffer = ProgressBuffer::new(ProgressSource {
        call_id: None,
        name: format!("{}\u{1b}[31m", "\u{754c}".repeat(200)),
        stream: ToolOutputStream::Stdout,
    });
    let message = buffer
        .push_delta(&format!("{}END", "\u{754c}".repeat(64 * 1024)))
        .expect("visible");
    assert!(message.len() <= BYTE_LIMIT);
    assert!(message.ends_with("END\n"));
    assert!(!message.contains('\u{1b}'));
}

#[test]
fn finishing_one_call_retains_other_calls_and_resets_reused_ids() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    let a = source("a", ToolOutputStream::Stdout);
    let b = source("b", ToolOutputStream::Stdout);
    append_tool_progress(harness.app_mut(), a.clone(), "old\u{1b}]");
    append_tool_progress(harness.app_mut(), b.clone(), "other\u{1b}[3");
    harness
        .app_mut()
        .tool_progress
        .finish(ProgressCompletion::CallId("a"));
    assert_eq!(harness.app().tool_progress.sources.len(), 1);
    append_tool_progress(harness.app_mut(), a, "new");
    append_tool_progress(harness.app_mut(), b, "1m-tail");
    let entries = &harness.app().active_turn.entries;
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].message, "bash stdout:\nold\n");
    assert_eq!(entries[1].message, "bash stdout:\nother-tail\n");
    assert_eq!(entries[2].message, "bash stdout:\nnew\n");
}

#[test]
fn reset_and_commit_drop_all_stream_state() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
    let input = source("a", ToolOutputStream::Stdout);
    append_tool_progress(harness.app_mut(), input.clone(), "old\u{1b}]");
    harness.app_mut().finalize_active_turn();
    assert!(harness.app().tool_progress.sources.is_empty());
    append_tool_progress(harness.app_mut(), input.clone(), "new\u{1b}]");
    harness.app_mut().reset_transcript();
    assert!(harness.app().tool_progress.sources.is_empty());
    append_tool_progress(harness.app_mut(), input, "fresh");
    assert_eq!(
        harness.app().active_turn.entries[0].message,
        "bash stdout:\nfresh\n"
    );
}

#[test]
fn identity_free_completion_cannot_retire_identified_calls() {
    let mut state = ToolProgressState::default();
    state.push_delta(source("a", ToolOutputStream::Stdout), "first\u{1b}[3");
    state.push_delta(
        ProgressSource {
            call_id: None,
            name: "bash".into(),
            stream: ToolOutputStream::Stdout,
        },
        "legacy",
    );
    state.finish(ProgressCompletion::LegacyName("bash"));
    assert_eq!(state.sources.len(), 1);
    let (_, text) = state
        .push_delta(source("a", ToolOutputStream::Stdout), "1m-tail")
        .expect("identified stream retained");
    assert_eq!(text, "bash stdout:\nfirst-tail\n");
}

#[test]
fn structured_terminal_result_retires_both_call_and_terminal_ids() {
    use crate::runtime_control::{RuntimeEvent, ToolEvent};
    use crate::tui::runtime::apply_tui_event;
    use crate::tui::state::TuiEvent;
    use crate::tui::terminal_event::{
        TerminalEvent, TerminalOutputDeltaEvent, TerminalStream, TerminalTarget,
    };

    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    append_tool_progress(
        harness.app_mut(),
        source("call-a", ToolOutputStream::Stdout),
        "call\u{1b}]",
    );
    append_tool_progress(
        harness.app_mut(),
        source("other", ToolOutputStream::Stdout),
        "other",
    );
    apply_tui_event(
        harness.app_mut(),
        TuiEvent::Terminal(TerminalEvent::OutputDelta(TerminalOutputDeltaEvent {
            target: TerminalTarget::Pty,
            id: Some("terminal-a".into()),
            stream: TerminalStream::Stdout,
            chunk: "terminal\u{1b}]".into(),
        })),
    );
    assert_eq!(harness.app().tool_progress.sources.len(), 3);
    let bus = crate::runtime_event_bus::RuntimeEventBus::new(8);
    let mut events = bus.subscribe_control();
    bus.publish_control(RuntimeEvent::Tool(ToolEvent::Result {
        call_id: Some("call-a".into()),
        name: "pty_read".into(),
        content: serde_json::json!({"session_id": "terminal-a", "status": "completed"}).to_string(),
        is_error: false,
    }));
    apply_tui_event(
        harness.app_mut(),
        TuiEvent::Runtime(Box::new(events.try_recv().unwrap())),
    );
    let sources = &harness.app().tool_progress.sources;
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source.call_id.as_deref(), Some("other"));
}
