use std::io;

use crossterm::event::EventStream;
use ratatui::backend::CrosstermBackend;

use super::document::PreparedEdit;
use crate::tui::custom_terminal::Terminal;
use crate::tui::terminal_control::TerminalTarget;
use crate::tui::terminal_feedback::{TitleMode, restore_title, save_title};
use crate::tui::terminal_modes::TerminalModeGuard;

pub(in crate::tui) async fn edit_with_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    modes: &mut TerminalModeGuard,
    events: EventStream,
    edit: PreparedEdit,
) -> io::Result<(EventStream, anyhow::Result<String>)> {
    drop(events);
    terminal.finish_inline_viewport()?;
    modes.restore()?;
    let handoff = EditorTerminal::capture(modes.title_mode)?;
    #[cfg(unix)]
    let signals = super::signals::EditorSignals::ignore_parent_interrupts()?;
    let result = edit.run().await;
    #[cfg(unix)]
    drop(signals);
    handoff.restore()?;
    *modes = TerminalModeGuard::start(modes.title_mode)?;
    #[cfg(unix)]
    modes.monitor_resumed_tty()?;
    Ok((EventStream::new(), result))
}

// Also restores on cancellation/unwind, when mode reacquisition is not reached.
struct EditorTerminal {
    active: bool,
    #[cfg(unix)]
    cooked: CookedTerminal,
}

impl EditorTerminal {
    fn capture(title_mode: TitleMode) -> io::Result<Self> {
        let guard = Self {
            active: true,
            #[cfg(unix)]
            cooked: CookedTerminal::capture()?,
        };
        if title_mode == TitleMode::Enabled {
            save_title(TerminalTarget::from_environment(), &mut io::stdout())?;
        }
        Ok(guard)
    }

    fn restore(mut self) -> io::Result<()> {
        self.restore_inner()
    }

    fn restore_inner(&mut self) -> io::Result<()> {
        if !std::mem::replace(&mut self.active, false) {
            return Ok(());
        }
        #[cfg(unix)]
        let cooked = self.cooked.restore_inner();
        #[cfg(not(unix))]
        let cooked: io::Result<()> = Ok(());
        // Even a crashed editor can own an alternate screen. This TUI resumes
        // its primary screen; attempt this independently of termios errors.
        let screen = crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        if let Err(error) = &screen {
            log::warn!("Could not leave editor alternate screen: {error}");
        }
        let title = restore_title(&mut io::stdout());
        if let Err(error) = &title {
            log::warn!("Could not restore title after editor: {error}");
        }
        cooked.and(screen).and(title)
    }
}

impl Drop for EditorTerminal {
    fn drop(&mut self) {
        if let Err(error) = self.restore_inner() {
            log::warn!("Could not restore editor terminal: {error}");
        }
    }
}

#[cfg(unix)]
struct CookedTerminal {
    tty: std::fs::File,
    saved: nix::sys::termios::Termios,
    active: bool,
}

#[cfg(unix)]
impl CookedTerminal {
    fn capture() -> io::Result<Self> {
        let tty = std::fs::File::open("/dev/tty")?;
        let saved = nix::sys::termios::tcgetattr(&tty)?;
        Ok(Self {
            tty,
            saved,
            active: true,
        })
    }

    fn restore_inner(&mut self) -> io::Result<()> {
        if self.active {
            nix::sys::termios::tcsetattr(
                &self.tty,
                nix::sys::termios::SetArg::TCSANOW,
                &self.saved,
            )?;
            self.active = false;
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for CookedTerminal {
    fn drop(&mut self) {
        if let Err(error) = self.restore_inner() {
            log::warn!("Could not restore cooked terminal after external editor: {error}");
        }
    }
}
