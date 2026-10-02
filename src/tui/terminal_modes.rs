use std::io;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};

static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();

pub(super) struct TerminalModeGuard {
    active: bool,
}

impl TerminalModeGuard {
    pub(super) fn start() -> io::Result<Self> {
        Self::acquire_with(|| {
            enable_raw_mode()?;
            execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)
        })
    }

    // Arm restoration before the first fallible operation, including partial setup.
    fn acquire_with(initialize: impl FnOnce() -> io::Result<()>) -> io::Result<Self> {
        PANIC_HOOK.call_once(|| {
            let previous_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if TERMINAL_ACTIVE.load(Ordering::Acquire)
                    && let Err(error) = restore_terminal_modes()
                {
                    log::warn!("Failed to restore terminal before panic: {error}");
                }
                previous_hook(info);
            }));
        });
        TERMINAL_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| io::Error::other("the process terminal already has an active TUI"))?;
        let guard = Self { active: true };
        initialize()?;
        Ok(guard)
    }

    pub(super) fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        restore_terminal_modes()?;
        self.active = false;
        TERMINAL_ACTIVE.store(false, Ordering::Release);
        Ok(())
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            log::warn!("Failed to restore terminal on exit: {error}");
            TERMINAL_ACTIVE.store(false, Ordering::Release);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreAction {
    Mouse,
    BracketedPaste,
    RawMode,
    Cursor,
}

fn restore_terminal_modes() -> io::Result<()> {
    restore_all(|action| match action {
        RestoreAction::Mouse => execute!(io::stdout(), DisableMouseCapture),
        RestoreAction::BracketedPaste => execute!(io::stdout(), DisableBracketedPaste),
        RestoreAction::RawMode => disable_raw_mode(),
        RestoreAction::Cursor => execute!(io::stdout(), Show),
    })
}

// A failed output write must never prevent restoring the kernel's raw-mode state.
fn restore_all(mut apply: impl FnMut(RestoreAction) -> io::Result<()>) -> io::Result<()> {
    let mut first_error = None;
    for action in [
        RestoreAction::Mouse,
        RestoreAction::BracketedPaste,
        RestoreAction::RawMode,
        RestoreAction::Cursor,
    ] {
        if let Err(error) = apply(action) {
            log::warn!("Failed to restore terminal {action:?}: {error}");
            first_error.get_or_insert(error);
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "terminal_modes_tests.rs"]
mod tests;
