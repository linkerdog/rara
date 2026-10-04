use std::io::{self, Write};

use base64::{Engine, engine::general_purpose::STANDARD};

pub(super) const MAX_RAW_BYTES: usize = 100_000;

#[derive(Clone, Copy)]
pub(super) enum TerminalTarget {
    Direct,
    Tmux,
    Screen,
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
}

pub(super) fn write_osc52(
    text: &str,
    target: TerminalTarget,
    writer: &mut dyn Write,
) -> io::Result<()> {
    if text.len() > MAX_RAW_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "selection exceeds the terminal clipboard limit ({} bytes; max {MAX_RAW_BYTES})",
                text.len()
            ),
        ));
    }
    let encoded = STANDARD.encode(text.as_bytes());
    let sequence = match target {
        TerminalTarget::Direct => format!("\x1b]52;c;{encoded}\x07"),
        TerminalTarget::Tmux => format!("\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\"),
        TerminalTarget::Screen => format!("\x1bP\x1b]52;c;{encoded}\x07\x1b\\"),
    };
    writer.write_all(sequence.as_bytes())?;
    writer.flush()
}
