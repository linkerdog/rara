use std::io::{self, Read, Write};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use ratatui::backend::CrosstermBackend;

use crate::tui::custom_terminal::Terminal;
use crate::tui::event_loop::{EventSource, TerminalEventSource};
use crate::tui::terminal_feedback::TitleMode;
use crate::tui::terminal_modes::TerminalModeGuard;

struct ChildGuard(Box<dyn Child + Send + Sync>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(error) => log::warn!("Could not inspect editor fixture: {error}"),
        }
        if let Err(error) = self.0.kill() {
            log::warn!("Could not stop editor fixture: {error}");
        }
        if let Err(error) = self.0.wait() {
            log::warn!("Could not reap editor fixture: {error}");
        }
    }
}

struct Probe {
    chunks: Receiver<io::Result<Vec<u8>>>,
    output: String,
    cursor_replies: usize,
}

impl Probe {
    fn wait_for(&mut self, marker: &str, writer: &mut dyn Write) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.output.contains(marker) {
            let bytes = self
                .chunks
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("waiting for {marker}: {error}: {}", self.output))
                .expect("PTY read");
            self.output.push_str(&String::from_utf8_lossy(&bytes));
            let queries = self.output.matches("\x1b[6n").count();
            for _ in self.cursor_replies..queries {
                writer.write_all(b"\x1b[1;1R").unwrap();
                writer.flush().unwrap();
            }
            self.cursor_replies = queries;
        }
    }
}

#[test]
fn real_editor_handoff_restores_modes_input_and_size_on_success_and_failures() {
    for scenario in [
        "success",
        "unconfigured",
        "cancel",
        "nonzero",
        "missing",
        "read_failure",
        "raw_failure",
        "interrupt",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("editor script.sh");
        let cancel_socket = directory.path().join("cancel.sock");
        std::fs::write(&script, "export RARA_EDITOR_FILE=\"$1\"\nexec \"$RARA_EDITOR_EXE\" --exact tui::external_editor::pty_tests::editor_child --ignored --nocapture\n").unwrap();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                ..Default::default()
            })
            .unwrap();
        let before = pair.master.get_termios().unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut command = CommandBuilder::new(&executable);
        command.args([
            "--exact",
            "tui::external_editor::pty_tests::tui_child",
            "--ignored",
            "--nocapture",
        ]);
        command.env("RARA_EDITOR_EXE", &executable);
        command.env("RARA_EDITOR_SCENARIO", scenario);
        command.env("RARA_EDITOR_CANCEL_SOCKET", &cancel_socket);
        command.env("TERM", "xterm-256color");
        command.env("VISUAL", "");
        command.env(
            "EDITOR",
            if scenario == "unconfigured" {
                String::new()
            } else if scenario == "missing" {
                directory.path().join("no-editor").display().to_string()
            } else {
                format!("/bin/sh '{}'", script.display())
            },
        );
        let mut child = ChildGuard(pair.slave.spawn_command(command).unwrap());
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();
        let (sender, receiver) = channel();
        let output = std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if sender.send(Ok(buffer[..count].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        if sender.send(Err(error)).is_err() {
                            log::warn!("Editor fixture reader receiver closed");
                        }
                        break;
                    }
                }
            }
        });
        let mut probe = Probe {
            chunks: receiver,
            output: String::new(),
            cursor_replies: 0,
        };
        if !matches!(scenario, "missing" | "unconfigured") {
            probe.wait_for("EDITOR_READY", writer.as_mut());
            assert_eq!(
                pair.master.get_termios().unwrap(),
                before,
                "editor must own cooked terminal: {scenario}"
            );
            assert_eq!(probe.output.matches("\x1b[>1u").count(), 1);
            assert_eq!(probe.output.matches("\x1b[<1u").count(), 1);
            assert_eq!(probe.output.matches("\x1b[22;2t").count(), 2);
            assert_eq!(probe.output.matches("\x1b[23;2t").count(), 1);
            pair.master
                .resize(PtySize {
                    rows: 18,
                    cols: 60,
                    ..Default::default()
                })
                .unwrap();
            writer
                .write_all(if scenario == "interrupt" {
                    b"\x03"
                } else {
                    b"editor input\n"
                })
                .unwrap();
            writer.flush().unwrap();
        }
        if scenario == "cancel" {
            probe.wait_for("EDITOR_RAW_READY", writer.as_mut());
            // Cancel only after the editor has changed kernel terminal modes.
            std::os::unix::net::UnixStream::connect(&cancel_socket).unwrap();
            probe.wait_for("TUI_DONE", writer.as_mut());
            assert!(child.0.wait().unwrap().success(), "{}", probe.output);
            assert_eq!(pair.master.get_termios().unwrap(), before);
            assert!(probe.output.contains("\x1b[?1049l"));
            assert_eq!(probe.output.matches("\x1b[22;2t").count(), 2);
            assert_eq!(probe.output.matches("\x1b[23;2t").count(), 2);
        } else {
            let acquisitions = if scenario == "unconfigured" { 1 } else { 2 };
            let title_acquisitions = if scenario == "unconfigured" { 1 } else { 3 };
            probe.wait_for("TUI_AGAIN", writer.as_mut());
            assert_ne!(pair.master.get_termios().unwrap(), before);
            assert_eq!(probe.output.matches("\x1b[>1u").count(), acquisitions);
            assert_eq!(probe.output.matches("\x1b[<1u").count(), acquisitions - 1);
            if !matches!(scenario, "missing" | "unconfigured") {
                assert!(probe.output.contains("TUI_AGAIN_60x18"), "{}", probe.output);
            }
            writer.write_all(b"x").unwrap();
            writer.flush().unwrap();
            probe.wait_for("TUI_DONE", writer.as_mut());
            let status = child.0.wait().unwrap();
            assert!(status.success(), "{scenario}: {}", probe.output);
            assert_eq!(pair.master.get_termios().unwrap(), before);
            assert_eq!(probe.output.matches("\x1b[>1u").count(), acquisitions);
            assert_eq!(probe.output.matches("\x1b[<1u").count(), acquisitions);
            assert_eq!(
                probe.output.matches("\x1b[22;2t").count(),
                title_acquisitions
            );
            assert_eq!(
                probe.output.matches("\x1b[23;2t").count(),
                title_acquisitions
            );
        }
        if let Some(path) = probe
            .output
            .split("EDIT_PATH=")
            .nth(1)
            .and_then(|tail| tail.lines().next())
        {
            assert!(
                !std::path::Path::new(path.trim_end())
                    .parent()
                    .unwrap()
                    .exists(),
                "editor backups must be removed"
            );
        }
        output.join().unwrap();
    }
}

#[test]
#[ignore = "external-editor TUI subprocess fixture"]
#[expect(
    clippy::print_stdout,
    reason = "The isolated PTY child sends synchronization markers."
)]
fn tui_child() {
    let scenario = std::env::var("RARA_EDITOR_SCENARIO").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut modes = TerminalModeGuard::start(TitleMode::Enabled).unwrap();
        TerminalModeGuard::run_owner(async {
            let mut tui = crate::tui::testing::TuiHarness::new(Default::default()).unwrap();
            tui.app_mut().set_input("original draft".into());
            let (draft, request) = super::EditorDraft::capture(tui.app_mut());
            let before = tui.app().bottom_pane.saved_draft();
            let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout())).unwrap();
            terminal
                .draw_inline(|frame| {
                    frame.render_widget(
                        ratatui::widgets::Paragraph::new("original draft"),
                        frame.area(),
                    )
                })
                .unwrap();
            let mut source = TerminalEventSource::new(&mut modes);
            // Arm the real reader before releasing it, as the live loop does.
            assert!(
                tokio::time::timeout(Duration::from_millis(20), source.next_event())
                    .await
                    .is_err()
            );
            if scenario == "cancel" {
                let cancel = tokio::net::UnixListener::bind(
                    std::env::var_os("RARA_EDITOR_CANCEL_SOCKET").unwrap(),
                )
                .unwrap();
                let editing = source.edit_external(&mut terminal, request);
                tokio::pin!(editing);
                tokio::select! {
                    result = cancel.accept() => { result.unwrap(); },
                    result = &mut editing => panic!("editor exited before cancellation: {result:?}"),
                }
                return;
            }
            let result = source.edit_external(&mut terminal, request).await.unwrap();
            assert_eq!(result.is_ok(), scenario == "success");
            draft.finish(tui.app_mut(), result).await;
            if scenario == "success" {
                assert_eq!(tui.app().bottom_pane.input, "edited draft");
            } else {
                assert_eq!(tui.app().bottom_pane.saved_draft(), before);
            }
            let tty = std::fs::File::open("/dev/tty").unwrap();
            assert!(
                !nix::sys::termios::tcgetattr(&tty)
                    .unwrap()
                    .local_flags
                    .contains(nix::sys::termios::LocalFlags::ICANON)
            );
            terminal
                .draw_inline(|frame| {
                    frame.render_widget(
                        ratatui::widgets::Paragraph::new("returned draft"),
                        frame.area(),
                    )
                })
                .unwrap();
            let size = terminal.size().unwrap();
            println!("TUI_AGAIN_{}x{}", size.width, size.height);
            io::stdout().flush().unwrap();
            loop {
                let event = tokio::time::timeout(Duration::from_secs(10), source.next_event())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if matches!(event, Event::Key(key) if key.code == KeyCode::Char('x')) {
                    break;
                }
            }
            terminal.finish_inline_viewport().unwrap();
        })
        .await
        .unwrap();
        modes.restore().unwrap();
        println!("TUI_DONE");
        io::stdout().flush().unwrap();
    });
}

#[test]
#[ignore = "external-editor subprocess fixture"]
#[expect(
    clippy::print_stdout,
    reason = "The isolated editor sends synchronization markers."
)]
fn editor_child() {
    let path = std::env::var("RARA_EDITOR_FILE").unwrap();
    let scenario = std::env::var("RARA_EDITOR_SCENARIO").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original draft");
    std::fs::write(format!("{path}~"), "backup").unwrap();
    println!("EDIT_PATH={path}\nEDITOR_READY");
    io::stdout().flush().unwrap();
    let mut line = String::new();
    io::stdin().read_line(&mut line).unwrap();
    assert_eq!(line, "editor input\n");
    match scenario.as_str() {
        "success" => std::fs::write(path, "edited draft").unwrap(),
        "nonzero" => {
            std::fs::write(path, "must not replace draft").unwrap();
            std::process::exit(7);
        }
        "read_failure" => std::fs::remove_file(path).unwrap(),
        "raw_failure" | "cancel" => {
            let tty = std::fs::File::open("/dev/tty").unwrap();
            let mut attributes = nix::sys::termios::tcgetattr(&tty).unwrap();
            nix::sys::termios::cfmakeraw(&mut attributes);
            nix::sys::termios::tcsetattr(&tty, nix::sys::termios::SetArg::TCSANOW, &attributes)
                .unwrap();
            crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen).unwrap();
            print!("\x1b]2;Editor title\x07");
            println!("EDITOR_RAW_READY");
            io::stdout().flush().unwrap();
            if scenario == "cancel" {
                loop {
                    std::thread::park();
                }
            }
            std::process::exit(7);
        }
        _ => panic!("unexpected editor scenario {scenario}"),
    }
}
