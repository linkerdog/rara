use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

use super::{CapturedDiff, DiffCapture};

pub(super) struct GitDiffCapture;

struct CaptureLimits {
    diff_bytes: usize,
    stderr_bytes: usize,
    deadline: Duration,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            diff_bytes: 256 * 1024,
            stderr_bytes: 16 * 1024,
            deadline: Duration::from_secs(10),
        }
    }
}

#[async_trait::async_trait]
impl DiffCapture for GitDiffCapture {
    async fn capture(&self, cwd: &Path) -> anyhow::Result<CapturedDiff> {
        capture_git_diff(cwd, OsStr::new("git"), CaptureLimits::default()).await
    }
}

async fn capture_git_diff(
    cwd: &Path,
    program: &OsStr,
    limits: CaptureLimits,
) -> anyhow::Result<CapturedDiff> {
    tokio::time::timeout(limits.deadline, async {
        let mut diff = CapturedDiff::default();
        let mut remaining = limits.diff_bytes;
        for staged in [true, false] {
            let label = if staged {
                "git diff --staged"
            } else {
                "git diff"
            };
            let mut command = Command::new(program);
            command.args([
                "--no-pager",
                "--no-optional-locks",
                "-c",
                "core.fsmonitor=false",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--submodule=short",
            ]);
            if staged {
                command.arg("--staged");
            }
            if !cwd.as_os_str().is_empty() {
                command.current_dir(cwd);
            }
            let output = run_command(&mut command, remaining, limits.stderr_bytes)
                .await
                .with_context(|| format!("{label} failed"))?;
            remaining = remaining.saturating_sub(output.bytes.len());
            diff.truncated |= output.truncated;
            let mut bytes = output.bytes;
            // A byte cap may split the final UTF-8 code point. Invalid bytes
            // inside the captured prefix remain errors, never an empty diff.
            if output.truncated
                && let Err(error) = std::str::from_utf8(&bytes)
                && error.error_len().is_none()
            {
                bytes.truncate(error.valid_up_to());
            }
            let text = String::from_utf8(bytes)
                .with_context(|| format!("{label} output is not valid UTF-8"))?;
            if !diff.text.is_empty() && !text.is_empty() {
                diff.text.push('\n');
            }
            diff.text.push_str(&text);
        }
        Ok(diff)
    })
    .await
    .with_context(|| {
        format!(
            "Git diff collection timed out after {} seconds",
            limits.deadline.as_secs()
        )
    })?
}

struct CappedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_capped(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> anyhow::Result<CappedOutput> {
    let mut output = CappedOutput {
        bytes: Vec::new(),
        truncated: false,
    };
    let mut buffer = [0; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok(output);
        }
        let retained = count.min(limit.saturating_sub(output.bytes.len()));
        output.bytes.extend_from_slice(&buffer[..retained]);
        output.truncated |= retained < count;
    }
}

struct GitChild(Option<Child>);

impl Drop for GitChild {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        // The child is not reaped until its pipes close, so its process-group
        // identity cannot be reused while descendants still hold those pipes.
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            use nix::errno::Errno;
            use nix::sys::signal::{Signal, killpg};
            use nix::unistd::Pid;
            match killpg(Pid::from_raw(pid as i32), Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => {}
                Err(error) => log::warn!("Failed to terminate Git process group: {error}"),
            }
        }
        if let Err(error) = child.start_kill() {
            log::warn!("Failed to terminate Git helper: {error}");
        }
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    if let Err(error) = child.wait().await {
                        log::warn!("Failed to reap Git helper: {error}");
                    }
                });
            }
            Err(error) => log::warn!("Cannot schedule Git helper cleanup: {error}"),
        }
    }
}

async fn run_command(
    command: &mut Command,
    stdout_limit: usize,
    stderr_limit: usize,
) -> anyhow::Result<CappedOutput> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut guard = GitChild(Some(command.spawn().context("could not start Git")?));
    let child = guard.0.as_mut().context("Git helper is unavailable")?;
    let stdout = child.stdout.take().context("Git stdout is unavailable")?;
    let stderr = child.stderr.take().context("Git stderr is unavailable")?;
    let (stdout, stderr) = tokio::try_join!(
        read_capped(stdout, stdout_limit),
        read_capped(stderr, stderr_limit),
    )?;
    let status = child.wait().await.context("could not wait for Git")?;
    guard.0 = None;
    if !status.success() {
        let diagnostic = String::from_utf8_lossy(&stderr.bytes);
        let suffix = if stderr.truncated {
            " [stderr truncated]"
        } else {
            ""
        };
        anyhow::bail!("Git exited with {status}: {}{suffix}", diagnostic.trim());
    }
    if !stderr.bytes.is_empty() {
        let suffix = if stderr.truncated {
            " [stderr truncated]"
        } else {
            ""
        };
        log::warn!(
            "Git diff reported: {}{suffix}",
            String::from_utf8_lossy(&stderr.bytes).trim()
        );
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests;
