use std::io::{self, Write};
use std::process::{Command, Stdio};

use base64::{Engine, engine::general_purpose::STANDARD};

pub(crate) fn copy_text(text: &str) -> io::Result<()> {
    write_osc52(text)?;
    try_native_clipboard(text);
    Ok(())
}

fn write_osc52(text: &str) -> io::Result<()> {
    let encoded = STANDARD.encode(text.as_bytes());
    let sequence = if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\")
    } else if std::env::var_os("STY").is_some() {
        format!("\x1bP\x1b]52;c;{encoded}\x07\x1b\\")
    } else {
        format!("\x1b]52;c;{encoded}\x07")
    };
    let mut stdout = io::stdout();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

fn try_native_clipboard(text: &str) {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = text;

    #[cfg(target_os = "macos")]
    {
        let _ = pipe_to_command("pbcopy", &[], text);
    }

    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WAYLAND_DISPLAY").is_some()
            && pipe_to_command("wl-copy", &[], text).is_ok()
        {
            return;
        }
        if pipe_to_command("xclip", &["-selection", "clipboard"], text).is_ok() {
            return;
        }
        let _ = pipe_to_command("xsel", &["--clipboard", "--input"], text);
    }
}

fn pipe_to_command(program: &str, args: &[&str], text: &str) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}
