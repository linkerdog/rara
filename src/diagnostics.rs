//! CLI log routing follows terminal ownership; embedding runtimes own their logger.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use log::{Level, LevelFilter, Log, Metadata, Record};
use rara_persistence::redaction::redact_secrets;

const MAX_PENDING: usize = 256;
const MAX_MESSAGE_BYTES: usize = 8192;

static LOGGER: CliLogger = CliLogger {
    capture: Mutex::new(Weak::new()),
};
static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

pub(crate) fn initialize_cli_logging() -> anyhow::Result<()> {
    INITIALIZED
        .get_or_init(|| {
            log::set_logger(&LOGGER).map_err(|error| error.to_string())?;
            log::set_max_level(LevelFilter::Warn);
            Ok(())
        })
        .as_ref()
        .copied()
        .map_err(|error| anyhow::anyhow!("Could not install CLI diagnostics: {error}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Diagnostic {
    pub(crate) level: Level,
    pub(crate) message: String,
}

impl Diagnostic {
    fn new(level: Level, message: String) -> Self {
        let mut message = redact_secrets(message);
        if message.len() > MAX_MESSAGE_BYTES {
            const SUFFIX: &str = " [truncated]";
            message.truncate(message.floor_char_boundary(MAX_MESSAGE_BYTES - SUFFIX.len()));
            message.push_str(SUFFIX);
        }
        Self { level, message }
    }
}

#[derive(Default)]
struct PendingDiagnostics {
    records: VecDeque<Diagnostic>,
    last: Option<Diagnostic>,
    dropped: usize,
}

impl PendingDiagnostics {
    fn push(&mut self, diagnostic: Diagnostic) {
        // Keep this across drains: recording a failed transcript write must not
        // enqueue the same write failure forever.
        if self.last.as_ref() == Some(&diagnostic) {
            return;
        }
        self.last = Some(diagnostic.clone());
        if self.records.len() == MAX_PENDING {
            self.records.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.records.push_back(diagnostic);
    }

    fn drain(&mut self) -> Vec<Diagnostic> {
        let mut records: Vec<_> = self.records.drain(..).collect();
        if self.dropped != 0 {
            records.push(Diagnostic::new(
                Level::Warn,
                format!(
                    "Diagnostic queue overflow: {} earlier messages were omitted.",
                    std::mem::take(&mut self.dropped)
                ),
            ));
        }
        records
    }
}

struct CliLogger {
    capture: Mutex<Weak<Mutex<PendingDiagnostics>>>,
}

impl Log for CliLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let diagnostic = Diagnostic::new(record.level(), record.args().to_string());
        let capture = self
            .capture
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(pending) = capture.upgrade() {
            pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(diagnostic);
        } else {
            write_cli_diagnostic(diagnostic);
        }
    }

    fn flush(&self) {}
}

fn write_cli_diagnostic(diagnostic: Diagnostic) {
    // This CLI sink is only used outside terminal ownership. A closed stderr
    // has no recovery path; logging must not panic or report to itself.
    let _ = writeln!(
        std::io::stderr().lock(),
        "{}: {}",
        diagnostic.level,
        diagnostic.message
    );
}

#[derive(Clone)]
pub(crate) struct DiagnosticReader(Arc<Mutex<PendingDiagnostics>>);

impl DiagnosticReader {
    pub(crate) fn drain(&self) -> Vec<Diagnostic> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .drain()
    }
}

/// Own this before entering raw mode and drop it after terminal restoration.
pub(crate) struct TerminalDiagnostics {
    reader: DiagnosticReader,
}

impl TerminalDiagnostics {
    pub(crate) fn start() -> anyhow::Result<Self> {
        initialize_cli_logging()?;
        let mut capture = LOGGER
            .capture
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        anyhow::ensure!(
            capture.upgrade().is_none(),
            "Terminal diagnostics already have an owner"
        );
        let reader = DiagnosticReader(Arc::new(Mutex::new(PendingDiagnostics::default())));
        *capture = Arc::downgrade(&reader.0);
        Ok(Self { reader })
    }

    pub(crate) fn reader(&self) -> DiagnosticReader {
        self.reader.clone()
    }
}

impl Drop for TerminalDiagnostics {
    fn drop(&mut self) {
        let mut capture = LOGGER
            .capture
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *capture = Weak::new();
        // Holding the routing lock fences concurrent producers across handoff.
        for diagnostic in self.reader.drain() {
            write_cli_diagnostic(diagnostic);
        }
    }
}

#[cfg(test)]
mod tests;
