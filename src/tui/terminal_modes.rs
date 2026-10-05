use std::cell::Cell;
use std::future::{Future, poll_fn};
use std::io::{self, IsTerminal};
use std::pin::pin;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

use crossterm::{
    cursor::Show,
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture,
    },
    execute,
    terminal::{EndSynchronizedUpdate, disable_raw_mode, enable_raw_mode},
};

// These own the single process terminal, never session runtime handles. The
// hook chains only the hook present at first acquisition; later replacements
// remain the installing caller's responsibility.
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();
#[cfg(unix)]
static KEYBOARD_MODE_OWNED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static OWNER_STATE: Cell<OwnerState> = const { Cell::new(OwnerState::Inactive) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnerState {
    Inactive,
    Running,
    Panicked,
}

struct TerminalOwnerScope {
    previous: OwnerState,
}

impl TerminalOwnerScope {
    fn enter() -> Self {
        Self {
            previous: OWNER_STATE.replace(OwnerState::Running),
        }
    }

    fn panicked(&self) -> bool {
        OWNER_STATE.get() == OwnerState::Panicked
    }
}

impl Drop for TerminalOwnerScope {
    fn drop(&mut self) {
        OWNER_STATE.set(self.previous);
    }
}

pub(super) struct TerminalModeGuard {
    active: bool,
    #[cfg(unix)]
    resumed_tty: Option<std::fs::File>,
}

impl TerminalModeGuard {
    pub(super) fn start() -> io::Result<Self> {
        if !io::stdout().is_terminal() {
            return Err(io::Error::other("stdout is not a terminal"));
        }
        Self::acquire_with(|| {
            enable_raw_mode()?;
            execute!(
                io::stdout(),
                EnableBracketedPaste,
                EnableMouseCapture,
                EnableFocusChange
            )?;
            #[cfg(unix)]
            enable_keyboard_enhancement(io::stdout())?;
            Ok(())
        })
    }

    // Arm restoration before the first fallible operation, including partial setup.
    fn acquire_with(initialize: impl FnOnce() -> io::Result<()>) -> io::Result<Self> {
        PANIC_HOOK.call_once(|| {
            let previous_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let owner_panicked = OWNER_STATE.with(|state| {
                    if state.get() == OwnerState::Running {
                        state.set(OwnerState::Panicked);
                        true
                    } else {
                        false
                    }
                });
                if TERMINAL_ACTIVE.load(Ordering::Acquire)
                    && owner_panicked
                    && let Err(error) = restore_before_panic()
                {
                    log::warn!("Failed to restore terminal before panic: {error}");
                }
                previous_hook(info);
            }));
        });
        TERMINAL_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| io::Error::other("the process terminal already has an active TUI"))?;
        let guard = Self {
            active: true,
            #[cfg(unix)]
            resumed_tty: None,
        };
        let scope = TerminalOwnerScope::enter();
        initialize()?;
        if scope.panicked() {
            return Err(io::Error::other("the terminal initializer caught a panic"));
        }
        Ok(guard)
    }

    pub(super) async fn run_owner<F: Future>(future: F) -> io::Result<F::Output> {
        let mut future = pin!(future);
        poll_fn(|cx| {
            // Workers may run on this same thread while the owner yields Pending.
            let scope = TerminalOwnerScope::enter();
            let result = future.as_mut().poll(cx);
            if scope.panicked() {
                // Even an internally caught panic must not resume a restored UI.
                Poll::Ready(Err(io::Error::other("the TUI owner caught a panic")))
            } else {
                result.map(Ok)
            }
        })
        .await
    }

    pub(super) fn restore(&mut self) -> io::Result<()> {
        self.restore_with(restore_terminal_modes)
    }

    #[cfg(unix)]
    pub(super) fn monitor_resumed_tty(&mut self) -> io::Result<()> {
        self.resumed_tty = Some(std::fs::File::open("/dev/tty")?);
        self.maintain_raw_mode()
    }

    /// A shell may restore saved termios after SIGCONT and our initial setup.
    /// Keep checking on maintenance ticks: crossterm's cached flag cannot
    /// observe that late write, and no fixed delay establishes a safe boundary.
    pub(super) fn maintain_raw_mode(&self) -> io::Result<()> {
        #[cfg(unix)]
        if self.active
            && let Some(tty) = &self.resumed_tty
        {
            use nix::sys::termios::{cfmakeraw, tcgetattr};

            let observed = tcgetattr(tty)?;
            let mut raw = observed.clone();
            cfmakeraw(&mut raw);
            if observed != raw {
                disable_raw_mode()?;
                enable_raw_mode()?;
            }
        }
        Ok(())
    }

    // Isolates the ownership-consumption boundary for failure injection.
    fn restore_with(&mut self, restore: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let result = restore();
        TERMINAL_ACTIVE.store(false, Ordering::Release);
        result
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            log::warn!("Failed to restore terminal on exit: {error}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreAction {
    SynchronizedOutput,
    Keyboard,
    Mouse,
    BracketedPaste,
    Focus,
    RawMode,
    Cursor,
}

fn restore_terminal_modes() -> io::Result<()> {
    restore_all(|action| match action {
        RestoreAction::SynchronizedOutput => execute!(io::stdout(), EndSynchronizedUpdate),
        RestoreAction::Keyboard => {
            // The panic hook may already have popped this entry before Drop.
            #[cfg(unix)]
            if KEYBOARD_MODE_OWNED.swap(false, Ordering::AcqRel) {
                return execute!(io::stdout(), crossterm::event::PopKeyboardEnhancementFlags);
            }
            Ok(())
        }
        RestoreAction::Mouse => execute!(io::stdout(), DisableMouseCapture),
        RestoreAction::BracketedPaste => execute!(io::stdout(), DisableBracketedPaste),
        RestoreAction::Focus => execute!(io::stdout(), DisableFocusChange),
        RestoreAction::RawMode => disable_raw_mode(),
        RestoreAction::Cursor => execute!(io::stdout(), Show),
    })
}

#[cfg(unix)]
fn enable_keyboard_enhancement(mut output: impl io::Write) -> io::Result<()> {
    use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};

    // Avoid crossterm's support query, whose DA1 drain is unbounded.
    // Unsupported ANSI terminals ignore the progressive enable command.
    crossterm::queue!(
        output,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    // An incomplete command has not pushed an entry. Once accepted, however,
    // a failing flush may still have sent it, so cleanup must already be armed.
    KEYBOARD_MODE_OWNED.store(true, Ordering::Release);
    output.flush()
}

fn restore_before_panic() -> io::Result<()> {
    let modes = restore_terminal_modes();
    // Do this before delegating to the previous hook. Unwinding destructors
    // cannot safely move the cursor after panic diagnostics have been printed.
    let handoff = (|| {
        let (_, rows) = crossterm::terminal::size()?;
        execute!(
            io::stdout(),
            crossterm::cursor::MoveTo(0, rows.saturating_sub(1)),
            crossterm::style::SetAttribute(crossterm::style::Attribute::Reset),
            crossterm::style::ResetColor,
            crossterm::style::Print("\r\n")
        )
    })();
    match (modes, handoff) {
        (Err(error), Err(handoff)) => {
            log::warn!("Failed to hand off terminal before panic: {handoff}");
            Err(error)
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

// A failed output write must never prevent restoring the kernel's raw-mode state.
fn restore_all(mut apply: impl FnMut(RestoreAction) -> io::Result<()>) -> io::Result<()> {
    let mut first_error = None;
    for action in [
        RestoreAction::SynchronizedOutput,
        RestoreAction::Keyboard,
        RestoreAction::Mouse,
        RestoreAction::BracketedPaste,
        RestoreAction::Focus,
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

#[cfg(all(test, unix))]
#[path = "keyboard_protocol_tests.rs"]
mod keyboard_tests;
