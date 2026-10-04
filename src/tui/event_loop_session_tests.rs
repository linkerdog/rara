use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use rara_persistence::prompt_history::PromptHistoryStore;

use super::fixture_runtime;
use crate::oauth::OAuthManager;
use crate::tui::event_loop::{StartupResumeTarget, TuiStartupOptions, run_tui};

struct SessionChild {
    handle: Box<dyn Child + Send + Sync>,
    reaped: bool,
}

impl Drop for SessionChild {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if let Err(error) = self.handle.kill() {
            log::warn!("Failed to stop session fixture: {error}");
        }
        if let Err(error) = self.handle.wait() {
            log::warn!("Failed to reap session fixture: {error}");
        }
    }
}

#[test]
fn full_session_hands_off_the_shell_before_restoring_modes() {
    check_history_shutdown(HistoryFixture::Writable);
}

#[test]
fn full_session_surfaces_history_failure_after_restoring_modes() {
    check_history_shutdown(HistoryFixture::Unavailable);
}

#[derive(Clone, Copy, PartialEq)]
enum HistoryFixture {
    Writable,
    Unavailable,
}

fn check_history_shutdown(history: HistoryFixture) {
    use fs2::FileExt;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let store = PromptHistoryStore::new(&home);
    let lock = std::fs::File::create(home.join("prompt_history.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let mut lock = Some(lock);
    if history == HistoryFixture::Unavailable {
        std::fs::create_dir(store.path()).unwrap();
    }
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let before = pair.master.get_termios().unwrap();
    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "tui::event_loop::loop_tests::session_tests::full_session_child",
        "--ignored",
        "--nocapture",
    ]);
    command.cwd(dir.path());
    command.env("RARA_HOME", dir.path().join("home"));
    command.env("RARA_TEST_SESSION_ROOT", dir.path());
    command.env(
        "RARA_TEST_HISTORY_FAILURE",
        if history == HistoryFixture::Unavailable {
            "1"
        } else {
            "0"
        },
    );
    let mut child = SessionChild {
        handle: pair.slave.spawn_command(command).unwrap(),
        reaped: false,
    };
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader_task = std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            let read = reader.read(&mut buffer);
            let done = matches!(read, Ok(0) | Err(_));
            if sender
                .send(read.map(|count| buffer[..count].to_vec()))
                .is_err()
                || done
            {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut output = Vec::new();
    let mut quit_sent = false;
    let mut ended = false;
    while !ended {
        let bytes = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|error| {
                panic!(
                    "session output: {error}: {}",
                    String::from_utf8_lossy(&output)
                )
            })
            .unwrap();
        ended = bytes.is_empty();
        output.extend(bytes);
        if !quit_sent && output.windows(8).any(|part| part == b"\x1b[?2026l") {
            writer.write_all(b"/quit\r").unwrap();
            writer.flush().unwrap();
            quit_sent = true;
        }
        // The cursor-show command ends restoration, after raw mode is disabled.
        if lock.is_some()
            && let Some(restored) = output.windows(8).position(|part| part == b"\x1b[?2004l")
            && output[restored..]
                .windows(6)
                .any(|part| part == b"\x1b[?25h")
        {
            assert_eq!(pair.master.get_termios().unwrap(), before);
            assert!(
                child.handle.try_wait().unwrap().is_none(),
                "history cleanup must still be pending"
            );
            drop(lock.take());
        }
    }
    reader_task.join().unwrap();
    let status = loop {
        if let Some(status) = child.handle.try_wait().unwrap() {
            child.reaped = true;
            break status;
        }
        assert!(Instant::now() < deadline, "session fixture did not exit");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{}", String::from_utf8_lossy(&output));
    assert!(
        lock.is_none(),
        "history writes must wait until terminal restoration"
    );
    assert_eq!(pair.master.get_termios().unwrap(), before);
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("SESSION_RETURNED raw=false"), "{output}");
    let disable_paste = output.rfind("\x1b[?2004l").unwrap();
    let restore_start = output[..disable_paste].rfind("\x1b[?2026l").unwrap();
    let final_frame_end = output[..restore_start].rfind("\x1b[?2026l").unwrap() + 8;
    assert!(
        output[final_frame_end..restore_start].contains("\r\n"),
        "shell handoff must follow the final frame and precede mode restoration: {output}"
    );
    let mut parser = vt100::Parser::new(24, 100, 0);
    parser.process(&output.as_bytes()[..restore_start]);
    assert_eq!(parser.screen().cursor_position(), (23, 0));
    if history == HistoryFixture::Writable {
        let loaded = store.load().unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].text(), "/quit");
    }
}

#[test]
#[ignore = "isolated full TUI session fixture"]
#[expect(
    clippy::print_stdout,
    reason = "The isolated session reports its restored mode to its parent."
)]
fn full_session_child() {
    let root = std::path::PathBuf::from(
        std::env::var_os("RARA_TEST_SESSION_ROOT").expect("isolated session root"),
    );
    assert_eq!(std::env::current_dir().unwrap(), root);
    assert_eq!(std::env::var_os("RARA_HOME").unwrap(), root.join("home"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (config, client) = fixture_runtime(&root).await;
        let oauth = OAuthManager::new_for_config_dir(root.join("oauth")).unwrap();
        let result = run_tui(
            client,
            oauth,
            TuiStartupOptions {
                config,
                resume: StartupResumeTarget::Fresh,
                permission_override: None,
            },
        )
        .await;
        if std::env::var("RARA_TEST_HISTORY_FAILURE").unwrap() == "1" {
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains("flush prompt history on exit"), "{error}");
        } else {
            result.unwrap();
        }
    });
    println!(
        "SESSION_RETURNED raw={}",
        crossterm::terminal::is_raw_mode_enabled().unwrap()
    );
}
