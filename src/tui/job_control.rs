use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::EventStream;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::getpgrp;
use ratatui::backend::CrosstermBackend;

use super::custom_terminal::Terminal;
use super::terminal_modes::TerminalModeGuard;

/// Yield the terminal to the shell, then reacquire it after foreground resume.
pub(super) fn suspend(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    modes: &mut TerminalModeGuard,
    events: EventStream,
) -> io::Result<EventStream> {
    let resume_signal = ResumeSignal::listen()?;
    // Drop the reader before releasing terminal ownership. No input task may
    // remain armed while the shell reads commands from the same terminal.
    drop(events);
    terminal.finish_inline_viewport()?;
    modes.restore()?;
    resume_signal.received.store(false, Ordering::SeqCst);
    let stopped = killpg(getpgrp(), Signal::SIGTSTP).map_err(io::Error::from);
    if stopped.is_ok() {
        // killpg may return before another thread handles the group stop.
        // Never emit input-mode commands until SIGCONT confirms resumption.
        resume_signal.wait()?;
    }
    let resumed = TerminalModeGuard::start();
    match (stopped, resumed) {
        (Ok(()), Ok(guard)) => *modes = guard,
        (Err(error), Ok(guard)) => {
            *modes = guard;
            return Err(error);
        }
        (Err(error), Err(cleanup)) => {
            log::warn!("Failed to reacquire terminal after suspend error: {cleanup}");
            return Err(error);
        }
        (Ok(()), Err(error)) => return Err(error),
    }
    modes.monitor_resumed_tty()?;
    // draw_inline reserves fresh rows below the shell's actual cursor. It also
    // invalidates both content and blank cells at the latest terminal size.
    Ok(EventStream::new())
}

struct ResumeSignal {
    registration: signal_hook::SigId,
    received: Arc<AtomicBool>,
}

impl ResumeSignal {
    fn listen() -> io::Result<Self> {
        let received = Arc::new(AtomicBool::new(false));
        let registration =
            signal_hook::flag::register(signal_hook::consts::SIGCONT, Arc::clone(&received))?;
        Ok(Self {
            registration,
            received,
        })
    }

    fn wait(&self) -> io::Result<()> {
        // Bound work while runnable, not elapsed wall time: a stopped job may
        // remain in the shell indefinitely. Ignored/blocked stops must fail
        // with the terminal still restored instead of hanging or reacquiring.
        for _ in 0..100 {
            if self.received.load(Ordering::SeqCst) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.received.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "no SIGCONT received after requesting terminal suspension",
            ))
        }
    }
}

impl Drop for ResumeSignal {
    fn drop(&mut self) {
        if !signal_hook::low_level::unregister(self.registration) {
            log::warn!("Terminal resume signal registration was already removed");
        }
    }
}

#[cfg(test)]
#[path = "job_control_tests.rs"]
mod tests;
