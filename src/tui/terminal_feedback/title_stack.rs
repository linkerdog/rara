use std::io::{self, Write};
use std::sync::atomic::{AtomicU8, Ordering};

use crate::config::TuiTerminalConfig;
use crate::tui::terminal_control::TerminalTarget;

// Single process terminal ownership, shared only with its panic cleanup hook.
static TITLE_OWNER: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum TitleMode {
    Disabled,
    Enabled,
}

impl TitleMode {
    pub(in crate::tui) fn configured(config: &TuiTerminalConfig) -> Self {
        if config.title && std::env::var("TERM").as_deref() != Ok("dumb") {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

pub(in crate::tui) fn save_title(target: TerminalTarget, output: &mut dyn Write) -> io::Result<()> {
    target.write("\x1b[22;2t", output)?;
    // A completed command can escape a failing flush. A partial command must
    // not arm a pop of a stack entry that belongs to the caller's shell.
    TITLE_OWNER.store(target as u8, Ordering::Release);
    output.flush()
}

pub(in crate::tui) fn restore_title(output: &mut dyn Write) -> io::Result<()> {
    let target = match TITLE_OWNER.swap(0, Ordering::AcqRel) {
        1 => TerminalTarget::Direct,
        2 => TerminalTarget::Tmux,
        3 => TerminalTarget::Screen,
        _ => return Ok(()),
    };
    target.write("\x1b[23;2t", output)?;
    output.flush()
}
