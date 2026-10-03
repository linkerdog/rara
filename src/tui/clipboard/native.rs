use std::io;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Writes complete selections without blocking the UI. Dropping an in-flight
/// future must terminate its owned helper; the caller owns the overall deadline.
#[async_trait]
pub(super) trait NativeClipboard: Send + Sync {
    async fn copy(&self, text: &str) -> io::Result<()>;
}

pub(super) struct PlatformClipboard;

#[async_trait]
impl NativeClipboard for PlatformClipboard {
    async fn copy(&self, text: &str) -> io::Result<()> {
        let commands: Vec<(&str, &[&str])> = match std::env::consts::OS {
            "macos" => vec![("pbcopy", &[])],
            "linux" => {
                let mut commands = vec![
                    ("xclip", &["-selection", "clipboard"][..]),
                    ("xsel", &["--clipboard", "--input"][..]),
                ];
                if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    commands.insert(0, ("wl-copy", &[]));
                }
                commands
            }
            _ => Vec::new(),
        };
        let mut failures = Vec::new();
        for (program, args) in commands {
            match pipe_to_command(Command::new(program).args(args), text).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    log::warn!("Native clipboard helper {program} failed: {error}");
                    failures.push(format!("{program}: {error}"));
                }
            }
        }
        if failures.is_empty() {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no native clipboard helper for this platform",
            ))
        } else {
            Err(io::Error::other(failures.join("; ")))
        }
    }
}

pub(super) async fn pipe_to_command(command: &mut Command, text: &str) -> io::Result<()> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(io::Error::other("clipboard helper stdin is unavailable"));
    };
    stdin.write_all(text.as_bytes()).await?;
    stdin.shutdown().await?;
    drop(stdin);
    let status = child.wait().await?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "clipboard helper exited with {status}"
        )))
    }
}
