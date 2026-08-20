use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};
use tokio_util::sync::CancellationToken;

use super::cache::DiagnosticCache;
use super::protocol::{
    JsonRpcRequest, JsonRpcResponse, PendingResponses, ProtocolWriter, fail_pending,
    read_lsp_messages,
};
use super::types::{LspFailure, LspFailureKind, ServerKind};

const STDERR_TAIL_BYTES: usize = 16 * 1024;

pub(super) struct ServerRuntime {
    kind: ServerKind,
    writer: Arc<ProtocolWriter>,
    pending: PendingResponses,
    next_id: AtomicU64,
    documents: AsyncMutex<HashMap<PathBuf, i64>>,
    cancellation: CancellationToken,
    running: Arc<AtomicBool>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    exit_rx: watch::Receiver<Option<LspFailure>>,
}

impl ServerRuntime {
    pub(super) async fn initialize(
        &self,
        workspace_root: &Path,
        timeout: Duration,
    ) -> Result<(), LspFailure> {
        let root_uri = url::Url::from_directory_path(workspace_root).map_err(|()| {
            LspFailure::new(
                LspFailureKind::ProtocolError,
                format!(
                    "cannot convert workspace root to LSP URI: {}",
                    workspace_root.display()
                ),
                false,
            )
            .for_server(self.kind)
        })?;
        self.request(
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": root_uri.as_str(),
                "capabilities": {
                    "window": {
                        "workDoneProgress": true
                    },
                    "workspace": {
                        "configuration": true,
                        "workspaceFolders": true
                    },
                    "textDocument": {
                        "publishDiagnostics": {
                            "relatedInformation": true,
                            "versionSupport": true
                        },
                        "synchronization": {
                            "didSave": true,
                            "dynamicRegistration": false
                        }
                    }
                }
            }),
            timeout,
            LspFailureKind::InitializeTimeout,
        )
        .await?;
        self.send_notification("initialized", serde_json::json!({}))
            .await
    }

    pub(super) async fn sync_file(
        &self,
        file_path: &Path,
        diagnostics: &Mutex<DiagnosticCache>,
    ) -> Result<(), LspFailure> {
        let content = tokio::fs::read_to_string(file_path).await.map_err(|err| {
            LspFailure::new(
                LspFailureKind::FileReadFailed,
                format!("failed to read {} for LSP: {err}", file_path.display()),
                false,
            )
            .for_server(self.kind)
        })?;
        let uri = url::Url::from_file_path(file_path).map_err(|()| {
            LspFailure::new(
                LspFailureKind::ProtocolError,
                format!(
                    "cannot convert file path to LSP URI: {}",
                    file_path.display()
                ),
                false,
            )
            .for_server(self.kind)
        })?;
        let (method, version) = {
            let mut documents = self.documents.lock().await;
            match documents.get_mut(file_path) {
                Some(version) => {
                    *version = version.saturating_add(1);
                    ("textDocument/didChange", *version)
                }
                None => {
                    documents.insert(file_path.to_path_buf(), 1);
                    ("textDocument/didOpen", 1)
                }
            }
        };
        diagnostics
            .lock()
            .unwrap()
            .mark_expected_version(file_path.to_path_buf(), version);

        let params = if method == "textDocument/didOpen" {
            serde_json::json!({
                "textDocument": {
                    "uri": uri.as_str(),
                    "languageId": self.kind.language_id(),
                    "version": version,
                    "text": content
                }
            })
        } else {
            serde_json::json!({
                "textDocument": {
                    "uri": uri.as_str(),
                    "version": version
                },
                "contentChanges": [{"text": content}]
            })
        };
        self.send_notification(method, params).await
    }

    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
        timeout_kind: LspFailureKind,
    ) -> Result<JsonRpcResponse, LspFailure> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        if let Err(failure) = self.write_message(&request).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(failure);
        }

        let mut exit_rx = self.exit_rx.clone();
        if let Some(failure) = exit_rx.borrow().clone() {
            self.pending.lock().unwrap().remove(&id);
            return Err(failure);
        }
        let response = tokio::select! {
            response = tokio::time::timeout(timeout, receiver) => match response {
                Ok(Ok(response)) => response?,
                Ok(Err(err)) => {
                    return Err(LspFailure::new(
                        LspFailureKind::ProtocolError,
                        format!(
                            "{} response channel for request {id} closed: {err}",
                            self.kind.label()
                        ),
                        true,
                    )
                    .for_server(self.kind));
                }
                Err(_) => {
                    self.pending.lock().unwrap().remove(&id);
                    return Err(LspFailure::new(
                        timeout_kind,
                        format!(
                            "{} timed out waiting {:?} for LSP response id {id}",
                            self.kind.label(),
                            timeout
                        ),
                        true,
                    )
                    .for_server(self.kind));
                }
            },
            changed = exit_rx.changed() => {
                self.pending.lock().unwrap().remove(&id);
                if changed.is_ok()
                    && let Some(failure) = exit_rx.borrow().clone()
                {
                    return Err(failure);
                }
                return Err(LspFailure::new(
                    LspFailureKind::ProtocolError,
                    format!("{} exit observer closed", self.kind.label()),
                    true,
                )
                .for_server(self.kind));
            }
        };
        if let Some(error) = response.error.as_ref() {
            return Err(LspFailure::new(
                LspFailureKind::ProtocolError,
                format!(
                    "{} response id {id} failed with code {}: {}",
                    self.kind.label(),
                    error.code,
                    error.message
                ),
                true,
            )
            .for_server(self.kind));
        }
        Ok(response)
    }

    async fn send_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), LspFailure> {
        self.write_message(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    async fn write_message(&self, message: &impl serde::Serialize) -> Result<(), LspFailure> {
        self.writer.write_message(message).await
    }

    pub(super) fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub(super) fn exit_receiver(&self) -> watch::Receiver<Option<LspFailure>> {
        self.exit_rx.clone()
    }

    pub(super) fn observed_exit(&self) -> Option<LspFailure> {
        self.exit_rx.borrow().clone()
    }
}

impl Drop for ServerRuntime {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

pub(super) async fn spawn_server(
    kind: ServerKind,
    command: &[OsString],
    workspace_root: &Path,
    diagnostics: Arc<Mutex<DiagnosticCache>>,
    initialize_timeout: Duration,
) -> Result<Arc<ServerRuntime>, LspFailure> {
    let Some(program) = command.first() else {
        return Err(LspFailure::new(
            LspFailureKind::SpawnFailed,
            format!("{} has an empty command", kind.label()),
            false,
        )
        .for_server(kind));
    };
    let mut child = Command::new(program)
        .args(&command[1..])
        .current_dir(workspace_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| spawn_failure(kind, program, err))?;
    let stdin = child.stdin.take().ok_or_else(|| {
        LspFailure::new(
            LspFailureKind::SpawnFailed,
            format!("{} stdin pipe is unavailable", kind.label()),
            true,
        )
        .for_server(kind)
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        LspFailure::new(
            LspFailureKind::SpawnFailed,
            format!("{} stdout pipe is unavailable", kind.label()),
            true,
        )
        .for_server(kind)
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        LspFailure::new(
            LspFailureKind::SpawnFailed,
            format!("{} stderr pipe is unavailable", kind.label()),
            true,
        )
        .for_server(kind)
    })?;

    let pending = Arc::new(Mutex::new(HashMap::new()));
    let cancellation = CancellationToken::new();
    let running = Arc::new(AtomicBool::new(true));
    let stderr_tail = Arc::new(Mutex::new(Vec::new()));
    let (exit_tx, exit_rx) = watch::channel(None);

    let writer = Arc::new(ProtocolWriter::new(kind, stdin));
    tokio::spawn(read_lsp_messages(
        stdout,
        diagnostics,
        workspace_root.to_path_buf(),
        kind,
        pending.clone(),
        writer.clone(),
    ));
    let stderr_task = tokio::spawn(capture_stderr(stderr, stderr_tail.clone()));
    let supervisor_pending = pending.clone();
    let supervisor_cancel = cancellation.clone();
    let supervisor_running = running.clone();
    let supervisor_stderr = stderr_tail.clone();
    tokio::spawn(async move {
        let status = tokio::select! {
            status = child.wait() => status,
            () = supervisor_cancel.cancelled() => {
                if let Err(err) = child.kill().await
                    && err.kind() != io::ErrorKind::InvalidInput
                {
                    log::warn!("failed to stop {}: {err}", kind.label());
                }
                child.wait().await
            }
        };
        supervisor_running.store(false, Ordering::Release);
        let _ = tokio::time::timeout(Duration::from_millis(100), stderr_task).await;
        if supervisor_cancel.is_cancelled() {
            return;
        }
        let failure = match status {
            Ok(status) => process_exit_failure(
                kind,
                &status,
                String::from_utf8_lossy(&supervisor_stderr.lock().unwrap()).to_string(),
            ),
            Err(err) => LspFailure::new(
                LspFailureKind::ServerExited,
                format!("failed to wait for {}: {err}", kind.label()),
                true,
            )
            .for_server(kind),
        };
        fail_pending(&supervisor_pending, failure.clone());
        let _ = exit_tx.send(Some(failure));
    });

    let runtime = Arc::new(ServerRuntime {
        kind,
        writer,
        pending,
        next_id: AtomicU64::new(1),
        documents: AsyncMutex::new(HashMap::new()),
        cancellation,
        running,
        stderr_tail,
        exit_rx,
    });
    if let Err(mut failure) = runtime.initialize(workspace_root, initialize_timeout).await {
        if failure.stderr_tail.is_none() {
            let tail = String::from_utf8_lossy(&runtime.stderr_tail.lock().unwrap()).to_string();
            failure.stderr_tail = (!tail.trim().is_empty()).then_some(tail);
        }
        return Err(failure);
    }
    if !runtime.is_running() {
        return Err(runtime.observed_exit().unwrap_or_else(|| {
            LspFailure::new(
                LspFailureKind::ServerExited,
                format!("{} exited during initialization", kind.label()),
                true,
            )
            .for_server(kind)
        }));
    }
    Ok(runtime)
}

fn spawn_failure(kind: ServerKind, program: &OsString, err: io::Error) -> LspFailure {
    let (failure_kind, retryable) = if err.kind() == io::ErrorKind::NotFound {
        (LspFailureKind::BinaryMissing, false)
    } else {
        (LspFailureKind::SpawnFailed, true)
    };
    LspFailure::new(
        failure_kind,
        format!("failed to spawn {} ({:?}): {err}", kind.label(), program),
        retryable,
    )
    .for_server(kind)
}

fn process_exit_failure(
    kind: ServerKind,
    status: &std::process::ExitStatus,
    stderr_tail: String,
) -> LspFailure {
    #[cfg(unix)]
    let signal = status.signal();
    #[cfg(not(unix))]
    let signal = None;
    let message = match (status.code(), signal) {
        (Some(code), _) => format!("{} exited with code {code}", kind.label()),
        (None, Some(signal)) => format!("{} exited by signal {signal}", kind.label()),
        (None, None) => format!("{} exited with unknown status", kind.label()),
    };
    LspFailure::new(LspFailureKind::ServerExited, message, true)
        .for_server(kind)
        .with_process_status(status.code(), signal, stderr_tail)
}

async fn capture_stderr(mut stderr: impl AsyncRead + Unpin, tail: Arc<Mutex<Vec<u8>>>) {
    let mut buffer = [0_u8; 4096];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => return,
            Ok(read) => append_bounded_tail(&tail, &buffer[..read]),
            Err(err) => {
                log::warn!("failed to read LSP stderr: {err}");
                return;
            }
        }
    }
}

fn append_bounded_tail(tail: &Mutex<Vec<u8>>, chunk: &[u8]) {
    let mut tail = tail.lock().unwrap();
    tail.extend_from_slice(chunk);
    if tail.len() > STDERR_TAIL_BYTES {
        let remove = tail.len() - STDERR_TAIL_BYTES;
        tail.drain(..remove);
    }
}
