use std::io::{self, Read, Write};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode};
use crossterm::terminal::is_raw_mode_enabled;
use futures::StreamExt;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, getpgrp};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::Paragraph;

use super::super::custom_terminal::Terminal;
use super::super::terminal_modes::TerminalModeGuard;

struct PtyJob {
    shell: Box<dyn Child + Send + Sync>,
    group: Option<Pid>,
    reaped: bool,
}

impl Drop for PtyJob {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if let Some(group) = self.group {
            match killpg(group, Signal::SIGKILL) {
                Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                Err(error) => eprintln!("Failed to clean up PTY job: {error}"),
            }
        }
        if let Err(error) = self.shell.kill() {
            eprintln!("Failed to stop PTY shell: {error}");
        }
        if let Err(error) = self.shell.wait() {
            eprintln!("Failed to reap PTY shell: {error}");
        }
    }
}

struct OutputProbe {
    chunks: Receiver<io::Result<Vec<u8>>>,
    output: String,
    cursor_replies: usize,
    search_from: usize,
}

impl OutputProbe {
    fn wait_for(&mut self, marker: &str, writer: &mut dyn Write) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !self.output[self.search_from..].contains(marker) {
            let bytes = self
                .chunks
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("waiting for {marker}: {error}: {}", self.output))
                .expect("PTY read");
            self.output.push_str(&String::from_utf8_lossy(&bytes));
            let queries = self.output.matches("\x1b[6n").count();
            for _ in self.cursor_replies..queries {
                writer.write_all(b"\x1b[1;1R").expect("cursor report");
                writer.flush().expect("flush cursor report");
            }
            self.cursor_replies = queries;
        }
        self.search_from += self.output[self.search_from..]
            .find(marker)
            .expect("marker")
            + marker.len();
    }
}

#[test]
fn foreground_suspend_restores_shell_and_resumes_input_after_resize() {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            ..PtySize::default()
        })
        .expect("open PTY");
    let before = pair.master.get_termios().expect("initial termios");
    let mut command = CommandBuilder::new("bash");
    // Each fg is a separate interactive command. Bash abandons a compound
    // command (including a -c loop) when its foreground job stops again.
    command.args(["--noprofile", "--norc", "-i"]);
    command.env("PS1", "SHELL_PROMPT> ");
    command.env("PROMPT_COMMAND", "");
    command.env("HISTFILE", "/dev/null");
    command.env("INPUTRC", "/dev/null");
    command.env(
        "RARA_TEST_JOB_EXE",
        std::env::current_exe().expect("test executable"),
    );
    let mut job = PtyJob {
        shell: pair
            .slave
            .spawn_command(command)
            .expect("spawn job-control shell"),
        group: None,
        reaped: false,
    };
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("PTY output");
    let mut writer = pair.master.take_writer().expect("PTY input");
    let (chunks, receiver) = channel();
    let output_task = std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if chunks.send(Ok(buffer[..count].to_vec())).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    if chunks.send(Err(error)).is_err() {
                        eprintln!("PTY output receiver closed after a read failure");
                    }
                    break;
                }
            }
        }
    });
    let mut probe = OutputProbe {
        chunks: receiver,
        output: String::new(),
        cursor_replies: 0,
        search_from: 0,
    };
    probe.wait_for("SHELL_PROMPT> ", writer.as_mut());
    let shell_termios = pair
        .master
        .get_termios()
        .expect("interactive shell termios");
    writer.write_all(b"\"$RARA_TEST_JOB_EXE\" --exact tui::job_control::tests::suspend_child --ignored --nocapture\n").expect("start foreground fixture");
    writer.flush().expect("flush command");
    probe.wait_for("PTY_JOB_READY", writer.as_mut());
    let group = probe
        .output
        .split("PTY_JOB_GROUP=")
        .nth(1)
        .expect("job group marker")
        .split_whitespace()
        .next()
        .expect("job group")
        .parse::<i32>()
        .expect("numeric job group");
    assert!(group > 1);
    assert_ne!(
        group,
        getpgrp().as_raw(),
        "fixture must never signal the test runner"
    );
    job.group = Some(Pid::from_raw(group));

    for (cycle, cols, rows) in [(1, 60, 18), (2, 100, 28)] {
        probe.wait_for("SHELL_PROMPT> ", writer.as_mut());
        assert_eq!(
            pair.master.get_termios().expect("shell termios"),
            shell_termios
        );
        let since_frame = probe
            .output
            .rsplit("FRAME_READY")
            .next()
            .expect("frame output");
        for reset in [
            "\x1b[?2026l",
            "\x1b[?1000l",
            "\x1b[?1006l",
            "\x1b[?2004l",
            "\x1b[?1004l",
            "\x1b[?25h",
        ] {
            assert!(
                since_frame.contains(reset),
                "missing {reset:?}: {since_frame}"
            );
        }
        pair.master
            .resize(PtySize {
                rows,
                cols,
                ..PtySize::default()
            })
            .expect("resize while suspended");
        writer.write_all(b"fg %1\n").expect("allow shell fg");
        writer.flush().expect("flush fg");
        probe.wait_for(&format!("RESUMED_{cycle}_{cols}x{rows}"), writer.as_mut());
        // The resumed EventStream must still receive ordinary keys.
        writer.write_all(b"x").expect("resumed input");
        writer.flush().expect("flush resumed input");
        probe.wait_for(&format!("INPUT_RECEIVED_{cycle}"), writer.as_mut());
        assert_ne!(pair.master.get_termios().expect("resumed termios"), before);
        writer.write_all(b"y").expect("allow next suspend");
        writer.flush().expect("flush next suspend");
    }
    probe.wait_for("CHILD_DONE", writer.as_mut());
    probe.wait_for("SHELL_PROMPT> ", writer.as_mut());
    assert_eq!(
        pair.master.get_termios().expect("final shell termios"),
        shell_termios
    );
    writer.write_all(b"exit\n").expect("exit shell");
    writer.flush().expect("flush exit");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = job.shell.try_wait().expect("poll shell") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "shell did not exit: {}",
            probe.output
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{}", probe.output);
    assert_eq!(pair.master.get_termios().expect("final termios"), before);
    assert!(!probe.output.contains("\x1b[2J"));
    assert!(!probe.output.contains("\x1b[3J"));
    output_task.join().expect("PTY reader");
    job.group = None;
    job.reaped = true;
}

// Only the PTY's foreground job signals its own process group.
#[test]
#[ignore = "job-control subprocess fixture"]
fn suspend_child() {
    println!("PTY_JOB_GROUP={}\nPTY_JOB_READY", getpgrp());
    io::stdout().flush().expect("job identity");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("current-thread runtime");
    runtime.block_on(async {
        use nix::sys::termios::{SetArg, tcgetattr, tcsetattr};

        let tty = std::fs::File::open("/dev/tty").expect("controlling terminal");
        let cooked = tcgetattr(&tty).expect("cooked terminal state");
        let mut modes = TerminalModeGuard::start().expect("start modes");
        TerminalModeGuard::run_owner(async {
            let mut terminal =
                Terminal::new(CrosstermBackend::new(io::stdout())).expect("terminal");
            let mut events = EventStream::new();
            for cycle in 1..=2 {
                terminal
                    .draw_inline(|frame| {
                        frame.render_widget(Paragraph::new("preserved draft"), frame.area());
                    })
                    .expect("draw initial frame");
                println!("FRAME_READY");
                io::stdout().flush().expect("frame marker");
                // Start the reader worker before suspension, as the live loop does.
                let pending = tokio::time::timeout(Duration::from_millis(20), events.next()).await;
                assert!(
                    pending.is_err(),
                    "unexpected input before suspend: {pending:?}"
                );
                events =
                    super::suspend(&mut terminal, &mut modes, events).expect("suspend and resume");
                assert!(is_raw_mode_enabled().expect("resumed raw mode"));
                terminal
                    .draw_inline(|frame| {
                        frame.render_widget(Paragraph::new("preserved draft"), frame.area());
                    })
                    .expect("redraw after resume");
                let size = terminal.size().expect("resumed size");
                assert_eq!(terminal.viewport_area.width, size.width);
                assert_eq!(terminal.viewport_area.height, size.height);
                // Emulate a shell restoring saved termios after our setup.
                // The cached crossterm flag must not conceal this late write.
                tcsetattr(&tty, SetArg::TCSANOW, &cooked).expect("late shell mode restoration");
                assert!(is_raw_mode_enabled().expect("cached raw state"));
                println!("RESUMED_{cycle}_{}x{}", size.width, size.height);
                io::stdout().flush().expect("resume marker");
                let mut maintenance = tokio::time::interval(Duration::from_millis(166));
                for expected in ['x', 'y'] {
                    loop {
                        tokio::select! {
                            _ = maintenance.tick() => modes.maintain_raw_mode().expect("repair raw mode"),
                            event = events.next() => {
                                if let Event::Key(key) = event.expect("input stream").expect("input event") {
                                    assert_eq!(key.code, KeyCode::Char(expected));
                                    break;
                                }
                            }
                        }
                    }
                    if expected == 'x' {
                        println!("INPUT_RECEIVED_{cycle}");
                        io::stdout().flush().expect("input acknowledgement");
                    }
                }
            }
            terminal
                .finish_inline_viewport()
                .expect("final shell handoff");
        })
        .await
        .expect("terminal owner");
        modes.restore().expect("final restoration");
    });
    println!("CHILD_DONE");
}
