use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

use super::cache::{DiagnosticCache, parse_publish_diagnostics};
use super::types::{LspFailure, LspFailureKind, ServerKind};

#[derive(Serialize)]
pub(super) struct JsonRpcRequest {
    pub(super) jsonrpc: &'static str,
    pub(super) id: u64,
    pub(super) method: String,
    pub(super) params: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct JsonRpcNotification {
    method: Option<String>,
    params: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub(super) struct JsonRpcResponse {
    pub(super) id: u64,
    pub(super) error: Option<JsonRpcResponseError>,
}

#[derive(Deserialize, Debug)]
pub(super) struct JsonRpcResponseError {
    pub(super) code: i64,
    pub(super) message: String,
}

pub(super) type PendingResponse = Result<JsonRpcResponse, LspFailure>;
pub(super) type PendingResponses = Arc<Mutex<HashMap<u64, oneshot::Sender<PendingResponse>>>>;

pub(super) struct ProtocolWriter {
    server_kind: ServerKind,
    inner: AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>,
}

impl ProtocolWriter {
    pub(super) fn new(
        server_kind: ServerKind,
        writer: impl AsyncWrite + Send + Unpin + 'static,
    ) -> Self {
        Self {
            server_kind,
            inner: AsyncMutex::new(Box::new(writer)),
        }
    }

    pub(super) async fn write_message(
        &self,
        message: &(impl Serialize + ?Sized),
    ) -> Result<(), LspFailure> {
        let body = serde_json::to_vec(message).map_err(|err| {
            LspFailure::new(
                LspFailureKind::ProtocolError,
                format!(
                    "failed to encode {} LSP message: {err}",
                    self.server_kind.label()
                ),
                false,
            )
            .for_server(self.server_kind)
        })?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut writer = self.inner.lock().await;
        writer
            .write_all(header.as_bytes())
            .await
            .map_err(|err| self.write_failure("header", err))?;
        writer
            .write_all(&body)
            .await
            .map_err(|err| self.write_failure("body", err))?;
        writer
            .flush()
            .await
            .map_err(|err| self.write_failure("flush", err))
    }

    fn write_failure(&self, phase: &str, err: std::io::Error) -> LspFailure {
        LspFailure::new(
            LspFailureKind::ProtocolError,
            format!(
                "failed to write {} LSP {phase}: {err}",
                self.server_kind.label()
            ),
            true,
        )
        .for_server(self.server_kind)
    }
}

pub(super) async fn read_lsp_messages(
    stdout: impl AsyncRead + Unpin,
    diagnostics: Arc<Mutex<DiagnosticCache>>,
    workspace_root: PathBuf,
    server_kind: ServerKind,
    pending: PendingResponses,
    writer: Arc<ProtocolWriter>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let content_length = match read_content_length(&mut reader).await {
            Ok(Some(content_length)) => content_length,
            Ok(None) => return,
            Err(failure) => {
                log::warn!("{}", failure);
                fail_pending(&pending, failure);
                return;
            }
        };
        let mut body = vec![0; content_length];
        if let Err(err) = reader.read_exact(&mut body).await {
            let failure = LspFailure::new(
                LspFailureKind::ProtocolError,
                format!(
                    "{} closed stdout while reading a {content_length}-byte LSP message: {err}",
                    server_kind.label()
                ),
                true,
            )
            .for_server(server_kind);
            log::warn!("{}", failure);
            fail_pending(&pending, failure);
            return;
        }
        route_lsp_message(
            &body,
            &diagnostics,
            &workspace_root,
            server_kind,
            &pending,
            &writer,
        )
        .await;
    }
}

async fn read_content_length(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> Result<Option<usize>, LspFailure> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await.map_err(|err| {
            LspFailure::new(
                LspFailureKind::ProtocolError,
                format!("failed to read LSP header: {err}"),
                true,
            )
        })?;
        if read == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return content_length.map(Some).ok_or_else(|| {
                LspFailure::new(
                    LspFailureKind::ProtocolError,
                    "LSP message omitted Content-Length",
                    true,
                )
            });
        }
        if let Some(colon_pos) = trimmed.find(':') {
            let (name, value) = trimmed.split_at(colon_pos);
            if name.eq_ignore_ascii_case("Content-Length") {
                content_length = Some(value[1..].trim().parse::<usize>().map_err(|err| {
                    LspFailure::new(
                        LspFailureKind::ProtocolError,
                        format!("invalid LSP Content-Length: {err}"),
                        true,
                    )
                })?);
            }
        }
    }
}

pub(super) async fn route_lsp_message(
    body: &[u8],
    diagnostics: &Mutex<DiagnosticCache>,
    workspace_root: &std::path::Path,
    server_kind: ServerKind,
    pending: &PendingResponses,
    writer: &ProtocolWriter,
) {
    let value = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) => value,
        Err(err) => {
            log::warn!("{} sent invalid JSON-RPC: {err}", server_kind.label());
            return;
        }
    };
    let has_method = value.get("method").is_some();
    let has_id = value.get("id").is_some();

    if has_id && !has_method {
        let response = match serde_json::from_value::<JsonRpcResponse>(value) {
            Ok(response) => response,
            Err(err) => {
                log::warn!("{} sent invalid LSP response: {err}", server_kind.label());
                return;
            }
        };
        let sender = pending.lock().unwrap().remove(&response.id);
        if let Some(sender) = sender {
            let _ = sender.send(Ok(response));
        }
        return;
    }

    if has_method && has_id {
        respond_to_server_request(&value, workspace_root, writer).await;
        return;
    }

    if has_method && !has_id {
        let notification = match serde_json::from_value::<JsonRpcNotification>(value) {
            Ok(notification) => notification,
            Err(err) => {
                log::warn!(
                    "{} sent invalid LSP notification: {err}",
                    server_kind.label()
                );
                return;
            }
        };
        if notification.method.as_deref() == Some("textDocument/publishDiagnostics")
            && let Some((path, version, file_diagnostics)) =
                parse_publish_diagnostics(notification.params, workspace_root)
        {
            diagnostics
                .lock()
                .unwrap()
                .publish(path, version, file_diagnostics);
        }
    }
}

async fn respond_to_server_request(
    request: &serde_json::Value,
    workspace_root: &std::path::Path,
    writer: &ProtocolWriter,
) {
    let Some(id) = request.get("id").cloned() else {
        return;
    };
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return;
    };
    let result = match method {
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability"
        | "workspace/diagnostic/refresh" => Some(Value::Null),
        "workspace/configuration" => {
            let item_count = request
                .get("params")
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            Some(Value::Array(vec![Value::Null; item_count]))
        }
        "workspace/workspaceFolders" => {
            url::Url::from_directory_path(workspace_root)
                .ok()
                .map(|uri| {
                    serde_json::json!([{
                        "name": workspace_root
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("workspace"),
                        "uri": uri.to_string(),
                    }])
                })
        }
        _ => None,
    };
    let response = result.map_or_else(
        || {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unsupported LSP server request: {method}"),
                }
            })
        },
        |result| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            })
        },
    );
    if let Err(failure) = writer.write_message(&response).await {
        log::warn!("{failure}");
    }
}

pub(super) fn fail_pending(pending: &PendingResponses, failure: LspFailure) {
    let senders = std::mem::take(&mut *pending.lock().unwrap());
    for (_, sender) in senders {
        let _ = sender.send(Err(failure.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routes_interleaved_responses_by_request_id() {
        let diagnostics = Mutex::new(DiagnosticCache::default());
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (client, _server) = tokio::io::duplex(4_096);
        let writer = ProtocolWriter::new(ServerKind::RustAnalyzer, client);
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, second_rx) = oneshot::channel();
        pending.lock().unwrap().insert(1, first_tx);
        pending.lock().unwrap().insert(2, second_tx);

        route_lsp_message(
            br#"{"jsonrpc":"2.0","id":2,"result":{}}"#,
            &diagnostics,
            std::path::Path::new("/repo"),
            ServerKind::RustAnalyzer,
            &pending,
            &writer,
        )
        .await;
        route_lsp_message(
            br#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            &diagnostics,
            std::path::Path::new("/repo"),
            ServerKind::RustAnalyzer,
            &pending,
            &writer,
        )
        .await;

        assert_eq!(
            first_rx.await.expect("first response").expect("first").id,
            1
        );
        assert_eq!(
            second_rx
                .await
                .expect("second response")
                .expect("second")
                .id,
            2
        );
    }

    #[tokio::test]
    async fn answers_workspace_configuration_server_requests() {
        let diagnostics = Mutex::new(DiagnosticCache::default());
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (client, server) = tokio::io::duplex(4_096);
        let writer = ProtocolWriter::new(ServerKind::RustAnalyzer, client);

        route_lsp_message(
            br#"{"jsonrpc":"2.0","id":"config-1","method":"workspace/configuration","params":{"items":[{"section":"rust-analyzer"},{}]}}"#,
            &diagnostics,
            std::path::Path::new("/repo"),
            ServerKind::RustAnalyzer,
            &pending,
            &writer,
        )
        .await;

        let mut reader = BufReader::new(server);
        let content_length = read_content_length(&mut reader)
            .await
            .expect("response header")
            .expect("content length");
        let mut body = vec![0_u8; content_length];
        reader.read_exact(&mut body).await.expect("response body");
        let response: Value = serde_json::from_slice(&body).expect("response json");

        assert_eq!(response["id"], "config-1");
        assert_eq!(response["result"], serde_json::json!([null, null]));
    }

    #[tokio::test]
    async fn reads_content_length_case_insensitively() {
        let body = r#"{"jsonrpc":"2.0"}"#;
        let message = format!("Content-length: {}\r\n\r\n{body}", body.len());
        let mut reader = BufReader::new(message.as_bytes());

        assert_eq!(
            read_content_length(&mut reader).await.expect("header"),
            Some(body.len())
        );
    }
}
