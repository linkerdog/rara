use std::io;

use super::{RestoreAction, restore_all};

const RESTORE_ACTIONS: [RestoreAction; 7] = [
    RestoreAction::SynchronizedOutput,
    RestoreAction::Keyboard,
    RestoreAction::Mouse,
    RestoreAction::BracketedPaste,
    RestoreAction::Focus,
    RestoreAction::RawMode,
    RestoreAction::Cursor,
];

#[test]
fn cleanup_attempts_every_mode_after_each_possible_failure() {
    for failed_action in RESTORE_ACTIONS {
        let mut actions = Vec::new();
        let error = restore_all(|action| {
            actions.push(action);
            if action == failed_action {
                Err(io::Error::other("injected restoration failure"))
            } else {
                Ok(())
            }
        })
        .expect_err("injected failure must surface");
        assert_eq!(actions, RESTORE_ACTIONS);
        assert_eq!(error.to_string(), "injected restoration failure");
    }
}

#[test]
fn cleanup_returns_first_error_when_multiple_modes_fail() {
    let mut actions = Vec::new();
    let error = restore_all(|action| {
        actions.push(action);
        Err(io::Error::other(format!("{action:?}")))
    })
    .expect_err("restoration failures must surface");
    assert_eq!(actions, RESTORE_ACTIONS);
    assert_eq!(error.to_string(), "SynchronizedOutput");
}

#[test]
fn failed_guard_restoration_is_not_retried() {
    let mut guard = super::TerminalModeGuard {
        active: true,
        #[cfg(unix)]
        resumed_tty: None,
    };
    let calls = std::cell::Cell::new(0);
    guard
        .restore_with(|| {
            calls.set(calls.get() + 1);
            Err(io::Error::other("injected cleanup failure"))
        })
        .expect_err("cleanup failure must surface");
    guard
        .restore_with(|| {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .expect("repeat restore is a no-op");
    drop(guard);
    assert_eq!(calls.get(), 1, "failed cleanup must consume ownership");
}

#[cfg(unix)]
mod pty {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    use crossterm::terminal::{enable_raw_mode, is_raw_mode_enabled};
    use crossterm::{cursor::Hide, event::EnableBracketedPaste, execute};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    use super::super::TerminalModeGuard;
    use super::super::keyboard_tests as keyboard;

    const SCENARIO_ENV: &str = "RARA_TEST_TERMINAL_EXIT";

    enum OutputFailure {
        Write,
        Flush,
    }

    struct FailingKeyboardOutput {
        failure: OutputFailure,
        wrote_prefix: bool,
    }

    impl Write for FailingKeyboardOutput {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if matches!(self.failure, OutputFailure::Write) && !bytes.is_empty() {
                if self.wrote_prefix {
                    return Err(std::io::Error::other("injected keyboard write failure"));
                }
                self.wrote_prefix = true;
                return std::io::stdout().write(&bytes[..1]);
            }
            std::io::stdout().write(bytes)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            std::io::stdout().flush()?;
            Err(std::io::Error::other("injected keyboard flush failure"))
        }
    }

    #[test]
    fn terminal_modes_restore_on_exit_without_disabling_caught_workers() {
        for scenario in [
            "pipe",
            "normal",
            "error",
            "partial",
            "before_keyboard",
            "push_write",
            "push_flush",
            "panic",
            "init_panic",
            "caught",
            "worker",
            "repeat",
            "enhanced",
            "legacy",
        ] {
            let pair = native_pty_system()
                .openpty(PtySize::default())
                .expect("open PTY");
            let before = pair.master.get_termios().expect("initial PTY termios");
            let mut command =
                CommandBuilder::new(std::env::current_exe().expect("test executable"));
            command.args([
                "--exact",
                "tui::terminal_modes::tests::pty::terminal_modes_child",
                "--ignored",
                "--nocapture",
            ]);
            command.env(SCENARIO_ENV, scenario);
            let mut child = pair.slave.spawn_command(command).expect("spawn PTY child");
            drop(pair.slave);
            let mut reader = pair.master.try_clone_reader().expect("PTY output");
            let mut writer = pair.master.take_writer().expect("PTY input");
            let output_task = std::thread::spawn(move || {
                let mut output = String::new();
                let mut buffer = [0; 4096];
                let mut sent_input = false;
                loop {
                    let count = reader.read(&mut buffer).expect("read PTY output");
                    if count == 0 {
                        break;
                    }
                    output.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    if !sent_input && output.contains("KEYS_READY") {
                        if scenario == "enhanced" {
                            assert!(output.contains("\x1b[>1u"), "{output}");
                        }
                        writer
                            .write_all(&keyboard::input(scenario))
                            .expect("keyboard input");
                        writer.flush().expect("flush keyboard input");
                        sent_input = true;
                    }
                }
                output
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.try_wait().expect("poll PTY child") {
                    break status;
                }
                if Instant::now() >= deadline {
                    child.kill().expect("terminate stalled PTY child");
                    child.wait().expect("reap stalled PTY child");
                    panic!("terminal restoration scenario timed out: {scenario}");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let after = pair.master.get_termios().expect("restored PTY termios");
            let output = output_task.join().expect("PTY reader");
            assert!(status.success(), "{scenario}: {output}");
            assert_eq!(after, before, "{scenario}: kernel terminal modes");
            assert!(output.contains("raw_after=false"), "{scenario}: {output}");
            let entries = match scenario {
                "pipe" | "before_keyboard" | "push_write" => 0,
                "repeat" => 2,
                _ => 1,
            };
            assert_eq!(
                output.matches("\x1b[>1u").count(),
                entries,
                "{scenario}: {output}"
            );
            assert_eq!(
                output.matches("\x1b[<1u").count(),
                entries,
                "{scenario}: {output}"
            );
            assert!(
                !output.contains("\x1b[?u"),
                "startup must not wait for a terminal response"
            );
            if scenario == "normal" {
                assert!(
                    output.contains("\x1b[>1u"),
                    "keyboard disambiguation was not enabled: {output}"
                );
                assert!(
                    output.contains("\x1b[?1004h"),
                    "focus reporting was not enabled: {output}"
                );
            }
            for reset in [
                "\x1b[?2026l",
                "\x1b[?1000l",
                "\x1b[?1006l",
                "\x1b[?2004l",
                "\x1b[?1004l",
                "\x1b[?25h",
            ] {
                assert!(
                    scenario == "pipe" || output.contains(reset),
                    "missing {reset:?}: {scenario}: {output}"
                );
            }
            if matches!(scenario, "panic" | "init_panic" | "caught") {
                let before_hook = output.split("previous_hook_raw=").next().unwrap();
                assert_eq!(
                    before_hook.matches("\x1b[<1u").count(),
                    1,
                    "{scenario}: {output}"
                );
                assert!(
                    output.contains("previous_hook_raw=false"),
                    "{scenario}: {output}"
                );
            }
            if scenario == "panic" {
                let before_hook = output.split("previous_hook_raw=").next().unwrap();
                let size = PtySize::default();
                let mut parser = vt100::Parser::new(size.rows, size.cols, 100);
                parser.process(before_hook.as_bytes());
                assert_eq!(
                    parser.screen().cursor_position(),
                    (size.rows - 1, 0),
                    "panic diagnostics must start below the frame"
                );
                assert!(parser.screen().contents().contains("FRAME-BOTTOM"));
            }
            if scenario == "worker" {
                let before_hook = output.split("previous_hook_raw=").next().unwrap();
                assert!(
                    !before_hook.contains("\x1b[<1u"),
                    "worker must preserve keyboard reporting"
                );
                assert!(
                    output.contains("previous_hook_raw=true"),
                    "{scenario}: {output}"
                );
            }
        }
    }

    // Runs only in a subprocess; never changes the parent test runner's modes or hook.
    #[test]
    #[ignore = "terminal subprocess fixture"]
    #[expect(
        clippy::print_stdout,
        reason = "Isolated PTY children emit mode and readiness markers to their parent."
    )]
    fn terminal_modes_child() {
        let scenario = std::env::var(SCENARIO_ENV).expect("PTY scenario");
        std::panic::set_hook(Box::new(|_| {
            println!(
                "previous_hook_raw={}",
                is_raw_mode_enabled().expect("raw mode state")
            );
        }));
        match scenario.as_str() {
            "pipe" => {
                // Keep the pipe child inside this isolated PTY session so even
                // the pre-fix path cannot alter the parent runner's terminal.
                let output =
                    std::process::Command::new(std::env::current_exe().expect("test executable"))
                        .args([
                            "--exact",
                            "tui::terminal_modes::tests::pty::terminal_modes_child",
                            "--ignored",
                            "--nocapture",
                        ])
                        .env(SCENARIO_ENV, "pipe_child")
                        .stdin(std::process::Stdio::null())
                        .output()
                        .expect("pipe child");
                assert!(
                    !output.stdout.contains(&0x1b),
                    "non-TTY stdout: {}",
                    String::from_utf8_lossy(&output.stdout)
                );
                assert!(
                    output.status.success(),
                    "pipe child: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            "pipe_child" => {
                let error = TerminalModeGuard::start()
                    .err()
                    .expect("non-TTY startup error");
                assert_eq!(error.to_string(), "stdout is not a terminal");
            }
            "normal" => {
                let mut guard = TerminalModeGuard::start().expect("start terminal modes");
                execute!(std::io::stdout(), Hide).expect("hide cursor");
                assert!(is_raw_mode_enabled().expect("raw mode state"));
                guard.restore().expect("restore terminal modes");
                guard.restore().expect("idempotent restoration");
            }
            "error" => {
                let error = (|| -> std::io::Result<()> {
                    let _guard = TerminalModeGuard::start()?;
                    Err(std::io::Error::other("injected loop error"))
                })()
                .expect_err("loop error");
                assert_eq!(error.to_string(), "injected loop error");
            }
            "partial" | "before_keyboard" => {
                let error = TerminalModeGuard::acquire_with(|| {
                    enable_raw_mode()?;
                    execute!(std::io::stdout(), EnableBracketedPaste, Hide)?;
                    if scenario == "partial" {
                        super::super::enable_keyboard_enhancement(std::io::stdout())?;
                    }
                    Err(std::io::Error::other("injected startup error"))
                })
                .err()
                .expect("startup error");
                assert_eq!(error.to_string(), "injected startup error");
            }
            "panic" => {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("current-thread runtime");
                assert!(
                    std::panic::catch_unwind(|| {
                        runtime.block_on(async {
                            let _guard = TerminalModeGuard::start().expect("start terminal modes");
                            TerminalModeGuard::run_owner(async {
                                tokio::task::yield_now().await;
                                let (_, rows) = crossterm::terminal::size().expect("TTY size");
                                execute!(
                                    std::io::stdout(),
                                    crossterm::cursor::MoveTo(0, rows - 1),
                                    crossterm::style::Print("FRAME-BOTTOM"),
                                    crossterm::cursor::MoveTo(4, 1)
                                )
                                .expect("place frame and composer cursor");
                                panic!("injected loop panic");
                            })
                            .await
                            .expect("owner panics before returning");
                        });
                    })
                    .is_err()
                );
            }
            "init_panic" => {
                assert!(
                    std::panic::catch_unwind(|| {
                        TerminalModeGuard::acquire_with(|| {
                            enable_raw_mode()?;
                            super::super::enable_keyboard_enhancement(std::io::stdout())?;
                            panic!("injected initializer panic");
                        })
                    })
                    .is_err()
                );
            }
            "caught" => {
                let _guard = TerminalModeGuard::start().expect("start terminal modes");
                let error = futures::executor::block_on(TerminalModeGuard::run_owner(async {
                    assert!(std::panic::catch_unwind(|| panic!("caught owner panic")).is_err());
                    std::future::pending::<()>().await;
                }))
                .expect_err("caught owner panic must terminate the UI");
                assert_eq!(error.to_string(), "the TUI owner caught a panic");
                assert!(!is_raw_mode_enabled().expect("raw mode state"));
            }
            "worker" => {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("current-thread runtime");
                runtime.block_on(async {
                    let _guard = TerminalModeGuard::start().expect("start terminal modes");
                    TerminalModeGuard::run_owner(async {
                        let worker = tokio::spawn(async { panic!("injected worker panic") });
                        assert!(worker.await.expect_err("worker panic").is_panic());
                        assert!(is_raw_mode_enabled().expect("worker must preserve raw mode"));
                    })
                    .await
                    .expect("worker panic must not terminate the owner");
                });
            }
            "repeat" => {
                for _ in 0..2 {
                    let guard = TerminalModeGuard::start().expect("start terminal modes");
                    assert!(is_raw_mode_enabled().expect("raw mode state"));
                    drop(guard);
                }
            }
            "enhanced" | "legacy" => {
                let _guard = TerminalModeGuard::start().expect("start terminal modes");
                println!("KEYS_READY");
                std::io::stdout().flush().expect("flush keyboard readiness");
                keyboard::check_input(&scenario);
            }
            "push_write" | "push_flush" => {
                let error = TerminalModeGuard::acquire_with(|| {
                    enable_raw_mode()?;
                    super::super::enable_keyboard_enhancement(FailingKeyboardOutput {
                        failure: if scenario == "push_write" {
                            OutputFailure::Write
                        } else {
                            OutputFailure::Flush
                        },
                        wrote_prefix: false,
                    })
                })
                .err()
                .expect("keyboard setup failure");
                assert!(error.to_string().contains("injected keyboard"));
            }
            other => panic!("unknown PTY scenario: {other}"),
        }
        println!(
            "raw_after={}",
            is_raw_mode_enabled().expect("raw mode state")
        );
        std::io::stdout().flush().expect("flush PTY output");
    }
}
