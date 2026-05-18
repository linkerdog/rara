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
use std::sync::{Arc, Mutex};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[allow(dead_code)]
    jsonrpc: String,
    method: Option<String>,
    params: Option<serde_json::Value>,
    #[allow(dead_code)]
    id: Option<u64>,
}

// ---------------------------------------------------------------------------
// LspManager
// ---------------------------------------------------------------------------

/// Manages LSP server processes and diagnostics cache.
pub struct LspManager {
    servers: Mutex<HashMap<ServerKind, Option<LspConnection>>>,
    diagnostics: Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>,
    workspace_root: PathBuf,
}

struct LspConnection {
    process: Child,
    _reader: std::thread::JoinHandle<()>,
    writer: std::process::ChildStdin,
    next_id: u64,
}

impl LspManager {
    /// Creates a new LspManager for the given workspace root.
    /// No servers are started until first use.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            workspace_root,
        }
    }

    /// Returns diagnostics for a file. Starts the appropriate LSP server
    /// if needed (lazy initialization).
    pub fn diagnostics_for(&self, file_path: &Path) -> Result<Vec<Diagnostic>> {
        let kind = self.detect_server(file_path)?;
        self.ensure_server_running(kind)?;
        self.sync_file(file_path, kind)?;

        let diags = self.diagnostics.lock().unwrap();
        Ok(diags.get(file_path).cloned().unwrap_or_default())
    }

    /// Returns all current diagnostics formatted for System context injection.
    pub fn diagnostics_summary(&self) -> String {
        let diags = self.diagnostics.lock().unwrap();
        if diags.is_empty() {
            return String::new();
        }
        let mut lines = vec!["# LSP Diagnostics".to_string()];
        for (path, file_diags) in diags.iter() {
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

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    fn detect_server(&self, file_path: &Path) -> Result<ServerKind> {
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();

        for kind in &[
            ServerKind::RustAnalyzer,
            ServerKind::Gopls,
            ServerKind::TypeScript,
        ] {
            if kind.extensions().contains(&ext.as_str()) {
                let detect = self.workspace_root.join(kind.detect_file());
                if detect.exists() {
                    if server_available(*kind) {
                        return Ok(*kind);
                    }
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
        let diags = Arc::clone(&self.diagnostics);
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                // Read Content-Length header
                let mut header = String::new();
                if reader.read_line(&mut header).is_err() {
                    break;
                }
                let content_len: usize = match header
                    .trim()
                    .strip_prefix("Content-Length: ")
                    .and_then(|s| s.trim().parse().ok())
                {
                    Some(len) => len,
                    None => continue,
                };
                // Skip the \r\n separator line
                let mut sep = String::new();
                let _ = reader.read_line(&mut sep);
                // Read the JSON body
                let mut buf = vec![0u8; content_len];
                if reader.read_exact(&mut buf).is_err() {
                    break;
                }
                let body = String::from_utf8_lossy(&buf);
                if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(&body) {
                    if notif.method.as_deref() == Some("textDocument/publishDiagnostics") {
                        if let Some(params) = &notif.params {
                            if let Some(uri) = params["uri"].as_str() {
                                let file_path = uri.strip_prefix("file://").unwrap_or(uri);
                                let path = Path::new(file_path);
                                let mut new_diags: Vec<Diagnostic> = vec![];
                                if let Some(items) = params["diagnostics"].as_array() {
                                    for d in items {
                                        new_diags.push(Diagnostic {
                                            file: file_path.to_string(),
                                            line: d["range"]["start"]["line"].as_u64().unwrap_or(0)
                                                as u32,
                                            column: d["range"]["start"]["character"]
                                                .as_u64()
                                                .unwrap_or(0)
                                                as u32,
                                            severity: match d["severity"].as_u64() {
                                                Some(1) => DiagnosticSeverity::Error,
                                                Some(2) => DiagnosticSeverity::Warning,
                                                Some(3) => DiagnosticSeverity::Information,
                                                Some(4) => DiagnosticSeverity::Hint,
                                                _ => DiagnosticSeverity::Information,
                                            },
                                            message: d["message"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string(),
                                            code: d["code"]
                                                .as_str()
                                                .map(|s| s.to_string())
                                                .or_else(|| {
                                                    d["code"]["value"]
                                                        .as_str()
                                                        .map(|s| s.to_string())
                                                }),
                                        });
                                    }
                                }
                                if let Ok(mut cache) = diags.lock() {
                                    cache.insert(path.to_path_buf(), new_diags);
                                }
                            }
                        }
                    }
                }
            }
        });

        let stdin = child.stdin.take().unwrap();
        let mut conn = LspConnection {
            process: child,
            _reader: reader,
            writer: stdin,
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
        let _ = conn.send_request("initialize", init_params);
        // Per LSP spec, initialized must be sent AFTER the initialize
        // response. Since we don't parse the response yet, sleep briefly
        // to let the server process the request before notifying.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = conn.send_notification("initialized", serde_json::json!({}));

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
}

impl LspConnection {
    fn send_request(&mut self, method: &str, params: serde_json::Value) -> Result<()> {
        self.next_id += 1;
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: self.next_id,
            method: method.to_string(),
            params,
        };
        let body = serde_json::to_string(&req)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer.write_all(header.as_bytes())?;
        self.writer.write_all(body.as_bytes())?;
        self.writer.flush()?;
        Ok(())
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
}

/// Returns true if the LSP server command is available on PATH.
/// Results are cached per-process via OnceLock.
fn server_available(kind: ServerKind) -> bool {
    use std::sync::OnceLock;
    static RUST_ANALYZER_OK: OnceLock<bool> = OnceLock::new();
    static GOPLS_OK: OnceLock<bool> = OnceLock::new();
    static TS_OK: OnceLock<bool> = OnceLock::new();

    let lock = match kind {
        ServerKind::RustAnalyzer => &RUST_ANALYZER_OK,
        ServerKind::Gopls => &GOPLS_OK,
        ServerKind::TypeScript => &TS_OK,
    };

    *lock.get_or_init(|| {
        let cmd = kind.command()[0];
        Command::new(cmd).arg("--version").output().is_ok()
    })
}

fn kind_to_lang_id(kind: ServerKind) -> &'static str {
    match kind {
        ServerKind::RustAnalyzer => "rust",
        ServerKind::Gopls => "go",
        ServerKind::TypeScript => "typescript",
    }
}
