use std::io;

use super::{RestoreAction, restore_all};

const RESTORE_ACTIONS: [RestoreAction; 4] = [
    RestoreAction::Mouse,
    RestoreAction::BracketedPaste,
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
    assert_eq!(error.to_string(), "Mouse");
}

#[cfg(unix)]
mod pty {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    use crossterm::terminal::{enable_raw_mode, is_raw_mode_enabled};
    use crossterm::{cursor::Hide, event::EnableBracketedPaste, execute};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    use super::super::TerminalModeGuard;

    const SCENARIO_ENV: &str = "RARA_TEST_TERMINAL_EXIT";

    #[test]
    fn terminal_modes_restore_on_normal_error_partial_startup_and_panic_exits() {
        for scenario in ["normal", "error", "partial", "panic", "hook", "repeat"] {
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
            let output_task = std::thread::spawn(move || {
                let mut output = String::new();
                reader.read_to_string(&mut output).expect("read PTY output");
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
            for reset in ["\x1b[?1000l", "\x1b[?1006l", "\x1b[?2004l", "\x1b[?25h"] {
                assert!(
                    output.contains(reset),
                    "missing {reset:?}: {scenario}: {output}"
                );
            }
            if matches!(scenario, "panic" | "hook") {
                assert!(
                    output.contains("previous_hook_raw=false"),
                    "{scenario}: {output}"
                );
            }
        }
    }

    // Runs only inside the PTY subprocess; never changes the parent test runner's modes or hook.
    #[test]
    #[ignore = "PTY subprocess fixture"]
    fn terminal_modes_child() {
        let scenario = std::env::var(SCENARIO_ENV).expect("PTY scenario");
        std::panic::set_hook(Box::new(|_| {
            println!(
                "previous_hook_raw={}",
                is_raw_mode_enabled().expect("raw mode state")
            );
        }));
        match scenario.as_str() {
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
            "partial" => {
                let error = TerminalModeGuard::acquire_with(|| {
                    enable_raw_mode()?;
                    execute!(std::io::stdout(), EnableBracketedPaste, Hide)?;
                    Err(std::io::Error::other("injected startup error"))
                })
                .err()
                .expect("startup error");
                assert_eq!(error.to_string(), "injected startup error");
            }
            "panic" => {
                assert!(
                    std::panic::catch_unwind(|| {
                        let _guard = TerminalModeGuard::start().expect("start terminal modes");
                        panic!("injected loop panic");
                    })
                    .is_err()
                );
            }
            "hook" => {
                // The panic hook must restore modes before any guard can unwind.
                let _guard = TerminalModeGuard::start().expect("start terminal modes");
                assert!(std::panic::catch_unwind(|| panic!("injected task panic")).is_err());
                assert!(!is_raw_mode_enabled().expect("raw mode state"));
            }
            "repeat" => {
                for _ in 0..2 {
                    let guard = TerminalModeGuard::start().expect("start terminal modes");
                    assert!(is_raw_mode_enabled().expect("raw mode state"));
                    drop(guard);
                }
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
