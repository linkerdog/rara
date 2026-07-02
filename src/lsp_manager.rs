//! Language Server Protocol (LSP) integration.
//!
//! Implements `docs/features/lsp-integration.md`.
//! Auto-detects language servers for rust, go, typescript, and provides
//! diagnostics as both a tool (`lsp_diagnostics`) and automatic System
//! context injection.
//!
//! ## Architecture
//!
//! ```text
//! Agent calls lsp_diagnostics("src/main.rs")
//!    → LspManager::diagnostics_for(file) → cached vec<Diagnostic>
//!         ↑
//!    rust-analyzer (stdio) → publishDiagnostics notifications
//! ```

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Severity of an LSP diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// A single diagnostic from an LSP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub code: Option<String>,
}

/// Supported language servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerKind {
    RustAnalyzer,
    Gopls,
    TypeScript,
}

impl ServerKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust-analyzer",
            Self::Gopls => "gopls",
            Self::TypeScript => "typescript-language-server",
        }
    }

    fn command(&self) -> &'static [&'static str] {
        match self {
            Self::RustAnalyzer => &["rust-analyzer"],
            Self::Gopls => &["gopls"],
            Self::TypeScript => &["typescript-language-server", "--stdio"],
        }
    }

    fn extensions(&self) -> &'static [&'static str] {
        match self {
            Self::RustAnalyzer => &[".rs"],
            Self::Gopls => &[".go"],
            Self::TypeScript => &[".ts", ".tsx", ".js", ".jsx"],
        }
    }

    fn detect_file(&self) -> &'static str {
        match self {
            Self::RustAnalyzer => "Cargo.toml",
            Self::Gopls => "go.mod",
            Self::TypeScript => "package.json",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspServerStatus {
    pub name: String,
    pub detected: bool,
    pub checked: bool,
    pub available: bool,
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspStatusSnapshot {
    pub enabled: bool,
    pub servers: Vec<LspServerStatus>,
    pub diagnostic_file_count: usize,
    pub diagnostic_count: usize,
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct JsonRpcNotification {
    #[allow(dead_code)] // Serde deserialization field
    jsonrpc: String,
    method: Option<String>,
    params: Option<serde_json::Value>,
    #[allow(dead_code)] // Serde deserialization field
    id: Option<u64>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcResponse {
    #[allow(dead_code)] // Serde deserialization field
    jsonrpc: String,
    id: u64,
    #[allow(dead_code)] // Serde deserialization field
    result: Option<serde_json::Value>,
    error: Option<JsonRpcResponseError>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcResponseError {
    code: i64,
    message: String,
}

// ---------------------------------------------------------------------------
// LspManager
// ---------------------------------------------------------------------------

/// Manages LSP server processes and diagnostics cache.
pub struct LspManager {
    servers: Mutex<HashMap<ServerKind, Option<LspConnection>>>,
    diagnostics: Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>,
    last_error: Mutex<Option<String>>,
    workspace_root: PathBuf,
    enabled: bool,
}

struct LspConnection {
    process: Child,
    _reader: std::thread::JoinHandle<()>,
    writer: std::process::ChildStdin,
    response_rx: Receiver<JsonRpcResponse>,
    next_id: u64,
}

impl LspManager {
    /// Creates a new LspManager for the given workspace root.
    /// No servers are started until first use.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            last_error: Mutex::new(None),
            workspace_root,
            enabled: lsp_enabled_from_env(),
        }
    }

    /// Returns diagnostics for a file. Starts the appropriate LSP server
    /// if needed (lazy initialization).
    pub fn diagnostics_for(&self, file_path: &Path) -> Result<Vec<Diagnostic>> {
        if !self.enabled {
            bail!("LSP is disabled by RARA_LSP");
        }
        let file_path = self.resolve_file_path(file_path);
        let kind = self.detect_server(&file_path)?;
        self.ensure_server_running(kind)?;
        self.sync_file(&file_path, kind)?;

        let diags = self.diagnostics.lock().unwrap();
        Ok(diags.get(&file_path).cloned().unwrap_or_default())
    }

    /// Returns all current diagnostics formatted for System context injection.
    pub fn diagnostics_summary(&self) -> String {
        let diags = self.diagnostics.lock().unwrap();
        if diags.is_empty() {
            return String::new();
        }
        let mut lines = vec!["# LSP Diagnostics".to_string()];
        for file_diags in diags.values() {
            for d in file_diags {
                let sev = match d.severity {
                    DiagnosticSeverity::Error => "error",
                    DiagnosticSeverity::Warning => "warning",
                    DiagnosticSeverity::Information => "info",
                    DiagnosticSeverity::Hint => "hint",
                };
                let code = d
                    .code
                    .as_ref()
                    .map(|c| format!("[{c}]"))
                    .unwrap_or_default();
                lines.push(format!(
                    "  {}:{}:{} {}{} {}",
                    d.file,
                    d.line + 1,
                    d.column + 1,
                    sev,
                    code,
                    d.message
                ));
            }
        }
        lines.join("\n")
    }

    pub fn status_snapshot(&self) -> LspStatusSnapshot {
        let servers = self.servers.lock().unwrap();
        let diags = self.diagnostics.lock().unwrap();
        let diagnostic_file_count = diags.len();
        let diagnostic_count = diags.values().map(Vec::len).sum();
        let server_statuses = all_server_kinds()
            .iter()
            .map(|kind| {
                let detected = self.workspace_root.join(kind.detect_file()).exists();
                let availability = server_available(*kind);
                LspServerStatus {
                    name: kind.label().to_string(),
                    detected,
                    checked: true,
                    available: detected && availability,
                    running: servers.get(kind).is_some_and(Option::is_some),
                }
            })
            .collect();
        LspStatusSnapshot {
            enabled: self.enabled,
            servers: server_statuses,
            diagnostic_file_count,
            diagnostic_count,
            last_error: self.last_error.lock().unwrap().clone(),
        }
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    fn detect_server(&self, file_path: &Path) -> Result<ServerKind> {
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();

        for kind in all_server_kinds() {
            if kind.extensions().contains(&ext.as_str()) {
                let detect = self.workspace_root.join(kind.detect_file());
                if detect.exists() && server_available(kind) {
                    return Ok(kind);
                }
            }
        }
        bail!("no LSP server for {}", file_path.display())
    }

    fn ensure_server_running(&self, kind: ServerKind) -> Result<()> {
        let mut servers = self.servers.lock().unwrap();
        if servers.contains_key(&kind) {
            return Ok(());
        }

        let cmd = kind.command();
        let mut child = Command::new(cmd[0])
            .args(&cmd[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn {:?}", cmd))?;

        let stdout = child.stdout.take().unwrap();
        let diagnostics = self.diagnostics.clone();
        let workspace_root = self.workspace_root.clone();
        let (response_tx, response_rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            read_lsp_messages(stdout, diagnostics, workspace_root, response_tx);
        });

        let stdin = child.stdin.take().unwrap();
        let mut conn = LspConnection {
            process: child,
            _reader: reader,
            writer: stdin,
            response_rx,
            next_id: 1,
        };

        // Send initialize request
        let init_params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", self.workspace_root.display()),
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": {}
                }
            }
        });
        let initialize_id = match conn.send_request("initialize", init_params) {
            Ok(id) => id,
            Err(err) => {
                *self.last_error.lock().unwrap() = Some(err.to_string());
                return Err(err);
            }
        };
        if let Err(err) = conn.wait_for_response(initialize_id, Duration::from_secs(5)) {
            *self.last_error.lock().unwrap() = Some(err.to_string());
            return Err(err);
        }
        if let Err(err) = conn.send_notification("initialized", serde_json::json!({})) {
            *self.last_error.lock().unwrap() = Some(err.to_string());
            return Err(err);
        }

        servers.insert(kind, Some(conn));
        Ok(())
    }

    fn sync_file(&self, file_path: &Path, kind: ServerKind) -> Result<()> {
        let mut servers = self.servers.lock().unwrap();
        let conn = servers
            .get_mut(&kind)
            .and_then(|c| c.as_mut())
            .ok_or_else(|| anyhow!("server not running for {:?}", kind))?;

        let uri = format!("file://{}", file_path.display());
        let content = std::fs::read_to_string(file_path).unwrap_or_default();

        conn.send_notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": kind_to_lang_id(kind),
                    "version": 1,
                    "text": content,
                }
            }),
        )?;
        // Drop the mutex lock before sleeping so other threads
        // can access diagnostics while we wait for server responses.
        drop(servers);

        // Poll for diagnostics (simple approach: sleep briefly)
        std::thread::sleep(std::time::Duration::from_millis(300));

        Ok(())
    }

    fn resolve_file_path<'a>(&'a self, file_path: &'a Path) -> PathBuf {
        if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            self.workspace_root.join(file_path)
        }
    }
}

impl Drop for LspConnection {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl LspConnection {
    fn send_request(&mut self, method: &str, params: serde_json::Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let body = serde_json::to_string(&req)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer.write_all(header.as_bytes())?;
        self.writer.write_all(body.as_bytes())?;
        self.writer.flush()?;
        Ok(id)
    }

    fn send_notification(&mut self, method: &str, params: serde_json::Value) -> Result<()> {
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&notif)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer.write_all(header.as_bytes())?;
        self.writer.write_all(body.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    fn wait_for_response(&self, id: u64, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                bail!("timed out waiting for LSP response id {id}");
            }
            let remaining = deadline.saturating_duration_since(now);
            let response = self
                .response_rx
                .recv_timeout(remaining)
                .with_context(|| format!("wait for LSP response id {id}"))?;
            if response.id != id {
                continue;
            }
            if let Some(error) = response.error {
                bail!(
                    "LSP response id {id} failed with code {}: {}",
                    error.code,
                    error.message
                );
            }
            return Ok(());
        }
    }
}

/// Returns true if the LSP server command is available on PATH.
/// Results are cached per-process via OnceLock.
fn server_available(kind: ServerKind) -> bool {
    *server_availability_lock(kind).get_or_init(|| {
        let cmd = kind.command()[0];
        Command::new(cmd).arg("--version").output().is_ok()
    })
}

fn server_availability_lock(kind: ServerKind) -> &'static std::sync::OnceLock<bool> {
    use std::sync::OnceLock;
    static RUST_ANALYZER_OK: OnceLock<bool> = OnceLock::new();
    static GOPLS_OK: OnceLock<bool> = OnceLock::new();
    static TS_OK: OnceLock<bool> = OnceLock::new();

    match kind {
        ServerKind::RustAnalyzer => &RUST_ANALYZER_OK,
        ServerKind::Gopls => &GOPLS_OK,
        ServerKind::TypeScript => &TS_OK,
    }
}

const fn all_server_kinds() -> [ServerKind; 3] {
    [
        ServerKind::RustAnalyzer,
        ServerKind::Gopls,
        ServerKind::TypeScript,
    ]
}

fn lsp_enabled_from_env() -> bool {
    std::env::var("RARA_LSP")
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !(value == "0" || value == "false" || value == "off")
        })
        .unwrap_or(true)
}

fn kind_to_lang_id(kind: ServerKind) -> &'static str {
    match kind {
        ServerKind::RustAnalyzer => "rust",
        ServerKind::Gopls => "go",
        ServerKind::TypeScript => "typescript",
    }
}

fn read_lsp_messages(
    stdout: impl Read,
    diagnostics: Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>,
    workspace_root: PathBuf,
    response_tx: Sender<JsonRpcResponse>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let Some(content_length) = read_content_length(&mut reader) else {
            return;
        };
        let mut body = vec![0; content_length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        handle_lsp_message(&body, &diagnostics, &workspace_root, &response_tx);
    }
}

fn handle_lsp_message(
    body: &[u8],
    diagnostics: &Mutex<HashMap<PathBuf, Vec<Diagnostic>>>,
    workspace_root: &Path,
    response_tx: &Sender<JsonRpcResponse>,
) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return;
    };
    let has_method = value.get("method").is_some();
    let has_id = value.get("id").is_some();

    if has_id && !has_method {
        if let Ok(response) = serde_json::from_value::<JsonRpcResponse>(value) {
            let _ = response_tx.send(response);
        }
    } else if has_method && !has_id {
        let Ok(notification) = serde_json::from_value::<JsonRpcNotification>(value) else {
            return;
        };
        if notification.method.as_deref() == Some("textDocument/publishDiagnostics")
            && let Some((path, file_diags)) =
                parse_publish_diagnostics(notification.params, workspace_root)
        {
            diagnostics.lock().unwrap().insert(path, file_diags);
        }
    }
}

fn read_content_length(reader: &mut impl BufRead) -> Option<usize> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            return None;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return content_length;
        }
        if let Some(colon_pos) = trimmed.find(':') {
            let (name, value) = trimmed.split_at(colon_pos);
            if name.eq_ignore_ascii_case("Content-Length") {
                content_length = value[1..].trim().parse().ok();
            }
        }
    }
}

fn parse_publish_diagnostics(
    params: Option<serde_json::Value>,
    workspace_root: &Path,
) -> Option<(PathBuf, Vec<Diagnostic>)> {
    let params = params?;
    let uri = params.get("uri")?.as_str()?;
    let path = path_from_file_uri(uri)?;
    let diagnostics = params.get("diagnostics")?.as_array()?;
    let file = display_path_for_diagnostic(&path, workspace_root);
    let parsed = diagnostics
        .iter()
        .filter_map(|item| parse_diagnostic(item, &file))
        .collect::<Vec<_>>();
    Some((path, parsed))
}

fn parse_diagnostic(value: &serde_json::Value, file: &str) -> Option<Diagnostic> {
    let start = value.get("range")?.get("start")?;
    let line = start.get("line")?.as_u64()? as u32;
    let column = start.get("character")?.as_u64()? as u32;
    let severity = match value.get("severity").and_then(serde_json::Value::as_u64) {
        Some(1) => DiagnosticSeverity::Error,
        Some(2) => DiagnosticSeverity::Warning,
        Some(3) => DiagnosticSeverity::Information,
        Some(4) | None => DiagnosticSeverity::Hint,
        Some(_) => DiagnosticSeverity::Information,
    };
    let message = value.get("message")?.as_str()?.to_string();
    let code = value.get("code").and_then(|code| match code {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    });
    Some(Diagnostic {
        file: file.to_string(),
        line,
        column,
        severity,
        message,
        code,
    })
}

fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

fn display_path_for_diagnostic(path: &Path, workspace_root: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_publish_diagnostics_notification() {
        let workspace = PathBuf::from("/repo");
        let params = serde_json::json!({
            "uri": "file:///repo/src/main.rs",
            "diagnostics": [{
                "range": { "start": { "line": 4, "character": 8 }, "end": { "line": 4, "character": 12 } },
                "severity": 1,
                "message": "cannot find value `x` in this scope",
                "code": "E0425"
            }]
        });

        let (path, diagnostics) = parse_publish_diagnostics(Some(params), &workspace).unwrap();

        assert_eq!(path, PathBuf::from("/repo/src/main.rs"));
        assert_eq!(
            diagnostics,
            vec![Diagnostic {
                file: "src/main.rs".to_string(),
                line: 4,
                column: 8,
                severity: DiagnosticSeverity::Error,
                message: "cannot find value `x` in this scope".to_string(),
                code: Some("E0425".to_string()),
            }]
        );
    }

    #[test]
    fn reads_lsp_content_length_header() {
        let body = r#"{"jsonrpc":"2.0"}"#;
        let message = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let mut reader = BufReader::new(message.as_bytes());

        assert_eq!(read_content_length(&mut reader), Some(body.len()));
    }

    #[test]
    fn reads_lsp_content_length_header_case_insensitively() {
        let body = r#"{"jsonrpc":"2.0"}"#;
        let message = format!("Content-length: {}\r\n\r\n{body}", body.len());
        let mut reader = BufReader::new(message.as_bytes());

        assert_eq!(read_content_length(&mut reader), Some(body.len()));
    }

    #[test]
    fn routes_lsp_response_messages_to_response_channel() {
        let diagnostics = Mutex::new(HashMap::new());
        let (tx, rx) = std::sync::mpsc::channel();
        let body = br#"{"jsonrpc":"2.0","id":7,"result":{"capabilities":{}}}"#;

        handle_lsp_message(body, &diagnostics, Path::new("/repo"), &tx);

        let response = rx.try_recv().unwrap();
        assert_eq!(response.id, 7);
        assert!(response.error.is_none());
        assert!(diagnostics.lock().unwrap().is_empty());
    }

    #[test]
    fn ignores_lsp_server_request_messages() {
        let diagnostics = Mutex::new(HashMap::new());
        let (tx, rx) = std::sync::mpsc::channel();
        let body =
            br#"{"jsonrpc":"2.0","id":9,"method":"window/workDoneProgress/create","params":{}}"#;

        handle_lsp_message(body, &diagnostics, Path::new("/repo"), &tx);

        assert!(rx.try_recv().is_err());
        assert!(diagnostics.lock().unwrap().is_empty());
    }

    #[test]
    fn server_available_caches_result_via_once_lock() {
        // First call runs --version (or finds cached result)
        let a = server_available(ServerKind::RustAnalyzer);
        // Second call hits OnceLock, returns same bool
        let b = server_available(ServerKind::RustAnalyzer);
        assert_eq!(a, b);
    }
}
