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
            )
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
        let scope = TerminalOwnerScope::enter();
        initialize()?;
        if scope.panicked() {
            return Err(io::Error::other("the terminal initializer caught a panic"));
        }
        Ok(guard)
    }

    pub(super) async fn run_owner<F: Future>(&self, future: F) -> io::Result<F::Output> {
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
    Mouse,
    BracketedPaste,
    Focus,
    RawMode,
    Cursor,
}

fn restore_terminal_modes() -> io::Result<()> {
    restore_all(|action| match action {
        RestoreAction::SynchronizedOutput => execute!(io::stdout(), EndSynchronizedUpdate),
        RestoreAction::Mouse => execute!(io::stdout(), DisableMouseCapture),
        RestoreAction::BracketedPaste => execute!(io::stdout(), DisableBracketedPaste),
        RestoreAction::Focus => execute!(io::stdout(), DisableFocusChange),
        RestoreAction::RawMode => disable_raw_mode(),
        RestoreAction::Cursor => execute!(io::stdout(), Show),
    })
}

// A failed output write must never prevent restoring the kernel's raw-mode state.
fn restore_all(mut apply: impl FnMut(RestoreAction) -> io::Result<()>) -> io::Result<()> {
    let mut first_error = None;
    for action in [
        RestoreAction::SynchronizedOutput,
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
