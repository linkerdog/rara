use std::io::{self, Write};

use base64::{Engine, engine::general_purpose::STANDARD};

pub(super) use crate::tui::terminal_control::TerminalTarget;

pub(super) const MAX_RAW_BYTES: usize = 100_000;

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
    target.write(&format!("\x1b]52;c;{encoded}\x07"), writer)?;
    writer.flush()
}
