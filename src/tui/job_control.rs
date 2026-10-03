use std::io;

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
    // Drop the reader before releasing terminal ownership. No input task may
    // remain armed while the shell reads commands from the same terminal.
    drop(events);
    terminal.finish_inline_viewport()?;
    modes.restore()?;
    let stopped = killpg(getpgrp(), Signal::SIGTSTP).map_err(io::Error::from);
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

#[cfg(test)]
#[path = "job_control_tests.rs"]
mod tests;
