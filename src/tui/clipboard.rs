use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

mod native;
mod osc52;

use native::{NativeClipboard, PlatformClipboard};
use osc52::{TerminalTarget, write_osc52};

use super::state::NoticeLevel;

const NATIVE_COPY_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct ClipboardFeedback {
    pub level: NoticeLevel,
    pub message: String,
}

impl ClipboardFeedback {
    fn new(level: NoticeLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
        }
    }
}

struct ClipboardOptions {
    target: TerminalTarget,
    writer: Box<dyn Write + Send>,
    native: Option<Arc<dyn NativeClipboard>>,
    timeout: Duration,
}

struct CopyRequest {
    text: String,
    terminal: io::Result<()>,
}

struct CopyTask {
    handle: JoinHandle<io::Result<()>>,
    terminal: io::Result<()>,
}

pub(crate) struct Clipboard {
    options: ClipboardOptions,
    active: Option<CopyTask>,
    pending: Option<CopyRequest>,
}

impl Clipboard {
    pub(crate) fn from_environment() -> Self {
        Self::new(ClipboardOptions {
            target: TerminalTarget::from_environment(),
            writer: Box::new(io::stdout()),
            native: (!super::terminal_ui::is_ssh_session())
                .then(|| Arc::new(PlatformClipboard) as Arc<dyn NativeClipboard>),
            timeout: NATIVE_COPY_TIMEOUT,
        })
    }

    fn new(options: ClipboardOptions) -> Self {
        Self {
            options,
            active: None,
            pending: None,
        }
    }

    pub(crate) fn request(&mut self, text: String) -> ClipboardFeedback {
        let terminal = write_osc52(&text, self.options.target, self.options.writer.as_mut());
        if let Err(error) = &terminal {
            log::warn!("Terminal clipboard request failed: {error}");
        }
        let request = CopyRequest { text, terminal };
        if self.options.native.is_none() {
            return terminal_notice(request.terminal);
        }
        if self.active.is_some() {
            self.pending = Some(request);
            ClipboardFeedback::new(NoticeLevel::Info, "Clipboard copy queued.")
        } else {
            self.start(request);
            ClipboardFeedback::new(
                NoticeLevel::Info,
                "Copying transcript selection to clipboard...",
            )
        }
    }

    fn start(&mut self, request: CopyRequest) {
        let Some(native) = self.options.native.clone() else {
            return;
        };
        let timeout = self.options.timeout;
        self.active = Some(CopyTask {
            terminal: request.terminal,
            handle: tokio::spawn(async move {
                tokio::time::timeout(timeout, native.copy(&request.text))
                    .await
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "native clipboard copy timed out")
                    })?
            }),
        });
    }

    /// Only await a completed task; slow clipboard helpers never stall input.
    pub(crate) async fn poll(&mut self) -> Option<ClipboardFeedback> {
        if !self
            .active
            .as_ref()
            .is_some_and(|task| task.handle.is_finished())
        {
            return None;
        }
        let task = self.active.take()?;
        let result = match task.handle.await {
            Ok(result) => result,
            Err(error) => Err(io::Error::other(format!("clipboard task failed: {error}"))),
        };
        if let Err(error) = &result {
            log::warn!("Native clipboard copy failed: {error}");
        }
        if let Some(request) = self.pending.take() {
            self.start(request);
            return None;
        }
        Some(match result {
            Ok(()) => ClipboardFeedback::new(
                NoticeLevel::Info,
                "Copied transcript selection to clipboard.",
            ),
            Err(error) => match task.terminal {
                Ok(()) => ClipboardFeedback::new(
                    NoticeLevel::Warning,
                    format!("Sent selection to terminal clipboard; native copy failed: {error}"),
                ),
                Err(terminal) => ClipboardFeedback::new(
                    NoticeLevel::Error,
                    format!("Failed to copy transcript selection: {terminal}; {error}"),
                ),
            },
        })
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        if let Some(task) = &self.active {
            task.handle.abort();
        }
    }
}

fn terminal_notice(result: io::Result<()>) -> ClipboardFeedback {
    match result {
        Ok(()) => ClipboardFeedback::new(
            NoticeLevel::Info,
            "Sent selection to terminal clipboard (acceptance depends on terminal policy).",
        ),
        Err(error) => ClipboardFeedback::new(
            NoticeLevel::Error,
            format!("Failed to copy transcript selection: {error}"),
        ),
    }
}

#[cfg(all(test, unix))]
mod process_tests;
#[cfg(test)]
mod tests;
