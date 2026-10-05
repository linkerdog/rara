use std::sync::Mutex;

use async_trait::async_trait;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{layout::Rect, text::Line};
use tokio::sync::{mpsc, oneshot};

use super::*;
use crate::tui::testing::TuiHarness;
use crate::tui::transcript_rows::TranscriptRows;

type CopyCall = (String, oneshot::Sender<io::Result<()>>);

struct ScriptedNative(mpsc::UnboundedSender<CopyCall>);

#[async_trait]
impl NativeClipboard for ScriptedNative {
    async fn copy(&self, text: &str) -> io::Result<()> {
        let (complete, done) = oneshot::channel();
        self.0
            .send((text.into(), complete))
            .map_err(|_| io::Error::other("copy observer closed"))?;
        done.await
            .map_err(|_| io::Error::other("scripted copy cancelled"))?
    }
}

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("output").extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn scripted_clipboard() -> (Clipboard, Output, mpsc::UnboundedReceiver<CopyCall>) {
    let (calls, receiver) = mpsc::unbounded_channel();
    let output = Output::default();
    let clipboard = Clipboard::new(ClipboardOptions {
        target: TerminalTarget::Direct,
        writer: Box::new(output.clone()),
        native: Some(Arc::new(ScriptedNative(calls))),
        timeout: NATIVE_COPY_TIMEOUT,
    });
    (clipboard, output, receiver)
}

#[tokio::test]
async fn copy_commands_use_the_last_completed_answer_and_markdown_code_blocks() {
    use crate::tui::message_role::MessageRole;
    use crate::tui::state::{RunningTask, TaskKind};

    let (clipboard, _, mut calls) = scripted_clipboard();
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().clipboard = Some(clipboard);
    let answer = "First:\n\n```rust\nlet first = 1;\n```\n\nLast:\n\n    print(42)\n";
    tui.app_mut().push_entry(MessageRole::Agent, answer);
    tui.app_mut().finalize_active_turn();
    tui.app_mut()
        .push_entry(MessageRole::Thinking, "hidden reasoning");
    tui.app_mut()
        .push_entry(MessageRole::Agent, "unfinished answer");
    let (_sender, receiver) = mpsc::unbounded_channel();
    tui.app_mut().bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(std::future::pending()),
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: None,
    });
    for (command, expected) in [("/copy", answer), ("/copy code", "print(42)\n")] {
        tui.app_mut().bottom_pane.input = command.into();
        tui.app_mut().sync_command_palette_with_input();
        tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .await
            .unwrap();
        let (text, complete) = calls.recv().await.unwrap();
        assert_eq!(text, expected);
        complete.send(Ok(())).unwrap();
        finish_task(tui.app_mut().clipboard.as_mut().unwrap()).await;
        assert!(tui.app().is_busy());
    }
    tui.app_mut()
        .bottom_pane
        .running_task
        .take()
        .unwrap()
        .handle
        .abort();
}

#[tokio::test]
async fn invalid_copy_and_absent_answers_leave_the_clipboard_untouched() {
    use crate::tui::message_role::MessageRole;
    let (clipboard, output, mut calls) = scripted_clipboard();
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().clipboard = Some(clipboard);
    for (command, notice) in [("/copy", "No completed"), ("/copy all", "Usage:")] {
        tui.app_mut().bottom_pane.input = command.into();
        tui.app_mut().sync_command_palette_with_input();
        tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .await
            .unwrap();
        assert!(
            tui.app()
                .bottom_pane
                .notice
                .as_deref()
                .unwrap()
                .contains(notice)
        );
    }
    tui.app_mut()
        .push_entry(MessageRole::Agent, "No code here.");
    tui.app_mut().bottom_pane.input = "/copy code".into();
    tui.app_mut().sync_command_palette_with_input();
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    assert!(
        tui.app()
            .bottom_pane
            .notice
            .as_deref()
            .unwrap()
            .contains("no code block")
    );
    assert!(calls.try_recv().is_err());
    assert!(output.0.lock().unwrap().is_empty());
}

async fn finish_task(clipboard: &mut Clipboard) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(3), async {
        while clipboard
            .active
            .as_ref()
            .is_some_and(|task| !task.handle.is_finished())
        {
            tokio::task::yield_now().await;
        }
        clipboard.poll().await
    })
    .await
    .expect("copy completion deadline")
}

#[tokio::test]
async fn stalled_clipboard_does_not_block_selection_dispatch_or_typing() {
    let (clipboard, _, mut calls) = scripted_clipboard();
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    tui.app_mut().clipboard = Some(clipboard);
    tui.app_mut().transcript_selection.update_snapshot(
        &TranscriptRows::from_visual_lines(vec![Line::from("selected text")]),
        Rect::new(0, 0, 13, 1),
        0,
    );
    for (kind, column) in [
        (MouseEventKind::Down(MouseButton::Left), 0),
        (MouseEventKind::Up(MouseButton::Left), 8),
    ] {
        let event = Event::Mouse(MouseEvent {
            kind,
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            !tokio::time::timeout(Duration::from_secs(1), tui.send_terminal_event(event))
                .await
                .expect("responsive selection")
                .expect("dispatch")
        );
    }
    let (copied, complete) = calls.recv().await.expect("native copy started");
    assert!(!copied.is_empty());
    assert!(
        !tui.press_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .await
            .expect("type while copy stalls")
    );
    assert_eq!(tui.app().bottom_pane.input, "a");
    assert!(
        tui.app_mut()
            .clipboard
            .as_mut()
            .expect("clipboard")
            .poll()
            .await
            .is_none()
    );
    complete.send(Ok(())).expect("complete copy");
    assert_eq!(
        finish_task(tui.app_mut().clipboard.as_mut().expect("clipboard"))
            .await
            .as_deref(),
        Some("Copied text to clipboard.")
    );
}

#[tokio::test]
async fn copy_queue_keeps_only_latest_pending_selection_and_suppresses_old_notice() {
    let (mut clipboard, _, mut calls) = scripted_clipboard();
    clipboard.request("first".into());
    let (first, complete) = calls.recv().await.expect("first call");
    assert_eq!(first, "first");
    clipboard.request("second".into());
    clipboard.request("latest".into());
    assert!(calls.try_recv().is_err(), "only one helper may be active");
    complete
        .send(Err(io::Error::other("obsolete failure")))
        .expect("finish first");
    assert!(
        finish_task(&mut clipboard).await.is_none(),
        "obsolete completion must not replace the latest notice"
    );
    let (latest, complete) = calls.recv().await.expect("next copy");
    assert_eq!(latest, "latest");
    complete.send(Ok(())).expect("finish latest");
    assert!(
        finish_task(&mut clipboard)
            .await
            .expect("notice")
            .starts_with("Copied")
    );
    assert!(clipboard.active.is_none());
    assert!(clipboard.pending.is_none());
}

#[test]
fn osc52_size_limit_uses_utf8_bytes_and_writes_nothing_on_overflow() {
    use base64::Engine;
    let exact = "\u{e9}".repeat(osc52::MAX_RAW_BYTES / 2);
    let mut output = Vec::new();
    write_osc52(&exact, TerminalTarget::Direct, &mut output).expect("exact boundary");
    let sequence = String::from_utf8(output).expect("ASCII sequence");
    let encoded = sequence
        .strip_prefix("\x1b]52;c;")
        .expect("OSC prefix")
        .strip_suffix('\x07')
        .expect("OSC suffix");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64"),
        exact.as_bytes()
    );
    let mut untouched = Vec::new();
    let error = write_osc52(&(exact + "x"), TerminalTarget::Direct, &mut untouched)
        .expect_err("byte overflow");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(untouched.is_empty());
}

#[test]
fn osc52_preserves_multiplexer_framing() {
    for (target, expected) in [
        (TerminalTarget::Direct, "\x1b]52;c;aGVsbG8=\x07"),
        (
            TerminalTarget::Tmux,
            "\x1bPtmux;\x1b\x1b]52;c;aGVsbG8=\x07\x1b\\",
        ),
        (TerminalTarget::Screen, "\x1bP\x1b]52;c;aGVsbG8=\x07\x1b\\"),
    ] {
        let mut output = Vec::new();
        write_osc52("hello", target, &mut output).expect("OSC write");
        assert_eq!(output, expected.as_bytes());
    }
}

#[tokio::test]
async fn oversized_selection_reaches_native_helper_intact() {
    let (mut clipboard, output, mut calls) = scripted_clipboard();
    let text = "x".repeat(osc52::MAX_RAW_BYTES + 1);
    clipboard.request(text.clone());
    let (copied, complete) = calls.recv().await.expect("native path");
    assert_eq!(copied, text);
    assert!(output.0.lock().expect("output").is_empty());
    complete.send(Ok(())).expect("copy completion");
    assert!(
        finish_task(&mut clipboard)
            .await
            .expect("notice")
            .starts_with("Copied")
    );
}

#[tokio::test]
async fn terminal_only_copy_does_not_claim_terminal_acceptance_or_spawn_work() {
    let mut clipboard = Clipboard::new(ClipboardOptions {
        target: TerminalTarget::Direct,
        writer: Box::new(Vec::new()),
        native: None,
        timeout: NATIVE_COPY_TIMEOUT,
    });
    assert!(
        clipboard
            .request("hello".into())
            .contains("acceptance depends on terminal policy")
    );
    assert!(
        clipboard
            .request("x".repeat(osc52::MAX_RAW_BYTES + 1))
            .contains("exceeds the terminal clipboard limit")
    );
    assert!(clipboard.active.is_none());
    assert!(clipboard.poll().await.is_none());
}

#[tokio::test]
async fn timeout_drops_stalled_native_copy_and_reports_terminal_fallback() {
    let (mut clipboard, _, mut calls) = scripted_clipboard();
    clipboard.options.timeout = Duration::from_millis(30);
    clipboard.request("hello".into());
    let (_, mut complete) = calls.recv().await.expect("stalled copy");
    let notice = finish_task(&mut clipboard).await.expect("timeout notice");
    assert!(notice.contains("Sent text to terminal clipboard"));
    assert!(notice.contains("timed out"));
    complete.closed().await;
}

#[tokio::test]
async fn dropping_session_cancels_owned_native_work() {
    let (mut clipboard, _, mut calls) = scripted_clipboard();
    clipboard.request("hello".into());
    let (_, mut complete) = calls.recv().await.expect("copy");
    drop(clipboard);
    tokio::time::timeout(Duration::from_secs(1), complete.closed())
        .await
        .expect("copy future cancelled");
}

#[cfg(unix)]
#[tokio::test]
async fn native_helper_failure_is_not_reported_as_success() {
    let error = native::pipe_to_command(
        tokio::process::Command::new("sh").args(["-c", "exit 17"]),
        "",
    )
    .await
    .expect_err("nonzero exit must fail");
    assert!(error.to_string().contains("17"));
}

#[tokio::test]
async fn native_spawn_failure_surfaces_without_a_clipboard_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = native::pipe_to_command(
        &mut tokio::process::Command::new(dir.path().join("missing-helper")),
        "selection",
    )
    .await
    .expect_err("missing helper");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

struct FailedWriter;

impl Write for FailedWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "terminal unavailable",
        ))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn terminal_failure_does_not_prevent_native_copy() {
    let (mut clipboard, _, mut calls) = scripted_clipboard();
    clipboard.options.writer = Box::new(FailedWriter);
    clipboard.request("selection".into());
    let (text, complete) = calls.recv().await.expect("native fallback");
    assert_eq!(text, "selection");
    complete.send(Ok(())).expect("copied");
    assert!(
        finish_task(&mut clipboard)
            .await
            .expect("notice")
            .starts_with("Copied")
    );
}

#[tokio::test]
async fn failure_of_both_clipboard_paths_surfaces_a_failure_notice() {
    let (mut clipboard, _, mut calls) = scripted_clipboard();
    clipboard.options.writer = Box::new(FailedWriter);
    clipboard.request("selection".into());
    let (_, complete) = calls.recv().await.expect("native fallback");
    complete
        .send(Err(io::Error::other("helper failed")))
        .expect("failed");
    let notice = finish_task(&mut clipboard).await.expect("notice");
    assert!(notice.contains("Failed to copy"));
    assert!(notice.contains("terminal unavailable"));
    assert!(notice.contains("helper failed"));
}
