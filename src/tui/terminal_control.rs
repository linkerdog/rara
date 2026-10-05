use std::io::{self, Write};

#[derive(Clone, Copy)]
pub(super) enum TerminalTarget {
    Direct = 1,
    Tmux = 2,
    Screen = 3,
}

impl TerminalTarget {
    pub(super) fn from_environment() -> Self {
        if std::env::var_os("TMUX").is_some() {
            Self::Tmux
        } else if std::env::var_os("STY").is_some() {
            Self::Screen
        } else {
            Self::Direct
        }
    }

    /// Passthrough must protect every inner ESC, including a string terminator.
    pub(super) fn write(self, sequence: &str, output: &mut dyn Write) -> io::Result<()> {
        let bytes = match self {
            Self::Direct => sequence.to_owned(),
            Self::Tmux => format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b")),
            Self::Screen => format!("\x1bP{sequence}\x1b\\"),
        };
        output.write_all(bytes.as_bytes())
    }
}
