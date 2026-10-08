use std::io;

use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

/// The foreground editor receives terminal interrupts; the parent waits for it.
pub(super) struct EditorSignals {
    previous: Vec<(Signal, SigAction)>,
}

impl EditorSignals {
    pub fn ignore_parent_interrupts() -> io::Result<Self> {
        let mut guard = Self {
            previous: Vec::new(),
        };
        let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
        for signal in [Signal::SIGINT, Signal::SIGQUIT] {
            // SAFETY: SIG_IGN is a kernel disposition, not a Rust callback. The
            // single terminal owner restores each previous disposition on Drop.
            let previous = unsafe { sigaction(signal, &ignore) }?;
            guard.previous.push((signal, previous));
        }
        Ok(guard)
    }
}

impl Drop for EditorSignals {
    fn drop(&mut self) {
        for (signal, previous) in self.previous.iter().rev() {
            // SAFETY: these are unchanged dispositions returned by sigaction
            // on acquisition, and remain valid for the lifetime of the process.
            if let Err(error) = unsafe { sigaction(*signal, previous) } {
                log::warn!("Could not restore {signal} after external editor: {error}");
            }
        }
    }
}

pub(super) fn configure_child(command: &mut tokio::process::Command) {
    // SAFETY: the child callback calls only async-signal-safe sigaction, with
    // stack-owned values and no allocation/locks before exec.
    unsafe {
        command.pre_exec(|| {
            let default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
            sigaction(Signal::SIGINT, &default)?;
            sigaction(Signal::SIGQUIT, &default)?;
            Ok(())
        });
    }
}
