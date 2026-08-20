use std::ffi::OsString;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerKind {
    RustAnalyzer,
    Gopls,
    TypeScript,
}

impl ServerKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust-analyzer",
            Self::Gopls => "gopls",
            Self::TypeScript => "typescript-language-server",
        }
    }

    pub(super) fn command(self) -> Vec<OsString> {
        match self {
            Self::RustAnalyzer => vec![OsString::from("rust-analyzer")],
            Self::Gopls => vec![OsString::from("gopls")],
            Self::TypeScript => vec![
                OsString::from("typescript-language-server"),
                OsString::from("--stdio"),
            ],
        }
    }

    pub(super) fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::RustAnalyzer => &[".rs"],
            Self::Gopls => &[".go"],
            Self::TypeScript => &[".ts", ".tsx", ".js", ".jsx"],
        }
    }

    pub(super) fn detect_file(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "Cargo.toml",
            Self::Gopls => "go.mod",
            Self::TypeScript => "package.json",
        }
    }

    pub(super) fn language_id(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust",
            Self::Gopls => "go",
            Self::TypeScript => "typescript",
        }
    }
}

pub(super) const fn all_server_kinds() -> [ServerKind; 3] {
    [
        ServerKind::RustAnalyzer,
        ServerKind::Gopls,
        ServerKind::TypeScript,
    ]
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LspServerPhase {
    NotStarted,
    Starting,
    Ready,
    Unavailable,
    Failed,
}

impl LspServerPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotStarted => "not started",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LspFailureKind {
    Disabled,
    UnsupportedFile,
    BinaryMissing,
    SpawnFailed,
    InitializeTimeout,
    ProtocolError,
    ServerExited,
    FileReadFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspFailure {
    pub kind: LspFailureKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    pub retryable: bool,
}

impl LspFailure {
    pub(super) fn new(kind: LspFailureKind, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            server: None,
            exit_code: None,
            signal: None,
            stderr_tail: None,
            retryable,
        }
    }

    pub(super) fn for_server(mut self, kind: ServerKind) -> Self {
        self.server = Some(kind.label().to_string());
        self
    }

    pub(super) fn with_process_status(
        mut self,
        exit_code: Option<i32>,
        signal: Option<i32>,
        stderr_tail: String,
    ) -> Self {
        self.exit_code = exit_code;
        self.signal = signal;
        self.stderr_tail = (!stderr_tail.trim().is_empty()).then_some(stderr_tail);
        self
    }
}

impl fmt::Display for LspFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(stderr_tail) = self.stderr_tail.as_deref() {
            write!(formatter, "; stderr: {stderr_tail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LspFailure {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspServerStatus {
    pub name: String,
    pub detected: bool,
    pub checked: bool,
    pub available: bool,
    pub running: bool,
    pub phase: LspServerPhase,
    pub last_failure: Option<LspFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspStatusSnapshot {
    pub enabled: bool,
    pub servers: Vec<LspServerStatus>,
    pub diagnostic_file_count: usize,
    pub diagnostic_count: usize,
    pub last_error: Option<String>,
    pub last_failure: Option<LspFailure>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticFreshness {
    Current,
    Cached,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspDiagnostics {
    pub diagnostics: Vec<Diagnostic>,
    pub freshness: DiagnosticFreshness,
}
