#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use super::manager::LspManagerOptions;
use super::{DiagnosticFreshness, LspFailureKind, LspManager, LspServerPhase, ServerKind};

const INITIALIZE_RESPONSE: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
const SERVER_CONFIGURATION_REQUEST: &str = r#"{"jsonrpc":"2.0","id":"workspace-config","method":"workspace/configuration","params":{"items":[{"section":"rust-analyzer"}]}}"#;

#[tokio::test]
async fn concurrent_callers_share_startup_and_documents_use_open_then_change() {
    let workspace = rust_workspace();
    let transcript = workspace.path().join("lsp-transcript.log");
    let server = write_script(
        workspace.path(),
        "fake-lsp.sh",
        &successful_server_script(Duration::from_millis(50)),
    );
    let manager = Arc::new(LspManager::for_test(
        workspace.path().to_path_buf(),
        LspManagerOptions::default().with_command(
            ServerKind::RustAnalyzer,
            vec![server.into_os_string(), transcript.clone().into_os_string()],
        ),
    ));

    let (first, second) = tokio::join!(
        manager.diagnostics_for(Path::new("src/main.rs")),
        manager.diagnostics_for(Path::new("src/main.rs")),
    );

    assert_eq!(
        first.expect("first diagnostics").freshness,
        DiagnosticFreshness::Pending
    );
    assert_eq!(
        second.expect("second diagnostics").freshness,
        DiagnosticFreshness::Pending
    );
    assert_eq!(manager.start_attempts(ServerKind::RustAnalyzer), 1);
    let status = manager.status_snapshot();
    let rust = status
        .servers
        .iter()
        .find(|server| server.name == "rust-analyzer")
        .expect("rust-analyzer status");
    assert_eq!(rust.phase, LspServerPhase::Ready);
    assert!(rust.running);

    manager
        .diagnostics_for(Path::new("src/main.rs"))
        .await
        .expect("third diagnostics");
    let transcript = wait_for_transcript(&transcript, "textDocument/didChange").await;
    assert_eq!(transcript.matches("textDocument/didOpen").count(), 1);
    assert_eq!(transcript.matches("textDocument/didChange").count(), 2);
    assert!(transcript.contains("workspace-config"));
    assert!(transcript.contains(r#""result":[null]"#));
}

#[tokio::test]
async fn initialize_timeout_is_typed_and_retryable() {
    let workspace = rust_workspace();
    let server = write_script(
        workspace.path(),
        "timeout-lsp.sh",
        "#!/bin/sh\nIFS= read -r first_header || exit 9\n/bin/sleep 5\n",
    );
    let manager = LspManager::for_test(
        workspace.path().to_path_buf(),
        LspManagerOptions::default()
            .with_command(ServerKind::RustAnalyzer, vec![server.into_os_string()])
            .with_initialize_timeout(Duration::from_millis(50))
            .with_retry_backoff(Duration::ZERO),
    );

    let failure = manager
        .diagnostics_for(Path::new("src/main.rs"))
        .await
        .expect_err("initialization should time out");

    assert_eq!(failure.kind, LspFailureKind::InitializeTimeout);
    assert!(failure.retryable);
    assert_eq!(
        rust_server_phase(&manager),
        LspServerPhase::Failed,
        "timeout must remain visible in status"
    );
}

#[tokio::test]
async fn early_exit_preserves_exit_code_and_stderr_tail() {
    let workspace = rust_workspace();
    let server = write_script(
        workspace.path(),
        "crashing-lsp.sh",
        "#!/bin/sh\nprintf 'synthetic crash\\n' >&2\nexit 17\n",
    );
    let manager = LspManager::for_test(
        workspace.path().to_path_buf(),
        LspManagerOptions::default()
            .with_command(ServerKind::RustAnalyzer, vec![server.into_os_string()]),
    );

    let failure = manager
        .diagnostics_for(Path::new("src/main.rs"))
        .await
        .expect_err("server should exit during initialization");

    assert_eq!(failure.kind, LspFailureKind::ServerExited);
    assert_eq!(failure.exit_code, Some(17));
    assert!(
        failure
            .stderr_tail
            .as_deref()
            .is_some_and(|tail| tail.contains("synthetic crash"))
    );
}

#[tokio::test]
async fn missing_binary_becomes_unavailable_without_repeated_probes() {
    let workspace = rust_workspace();
    let missing = workspace.path().join("missing-rust-analyzer");
    let manager = LspManager::for_test(
        workspace.path().to_path_buf(),
        LspManagerOptions::default()
            .with_command(ServerKind::RustAnalyzer, vec![missing.into_os_string()]),
    );

    for _ in 0..2 {
        let failure = manager
            .diagnostics_for(Path::new("src/main.rs"))
            .await
            .expect_err("binary should be unavailable");
        assert_eq!(failure.kind, LspFailureKind::BinaryMissing);
    }

    assert_eq!(manager.start_attempts(ServerKind::RustAnalyzer), 1);
    assert_eq!(rust_server_phase(&manager), LspServerPhase::Unavailable);
}

#[tokio::test]
async fn cancelled_startup_does_not_leave_the_slot_stuck_in_starting() {
    let workspace = rust_workspace();
    let transcript = workspace.path().join("lsp-transcript.log");
    let first_started = workspace.path().join("first-started");
    let server = write_script(
        workspace.path(),
        "cancel-then-start-lsp.sh",
        &cancel_then_success_server_script(),
    );
    let manager = Arc::new(LspManager::for_test(
        workspace.path().to_path_buf(),
        LspManagerOptions::default()
            .with_command(
                ServerKind::RustAnalyzer,
                vec![
                    server.into_os_string(),
                    transcript.into_os_string(),
                    first_started.clone().into_os_string(),
                ],
            )
            .with_retry_backoff(Duration::ZERO),
    ));
    let first_manager = manager.clone();
    let first = tokio::spawn(async move {
        first_manager
            .diagnostics_for(Path::new("src/main.rs"))
            .await
    });
    wait_for_path(&first_started).await;

    first.abort();
    assert!(first.await.expect_err("cancelled startup").is_cancelled());
    assert_eq!(rust_server_phase(&manager), LspServerPhase::Failed);

    let result = manager
        .diagnostics_for(Path::new("src/main.rs"))
        .await
        .expect("retry after cancellation");
    assert_eq!(result.freshness, DiagnosticFreshness::Pending);
    assert_eq!(manager.start_attempts(ServerKind::RustAnalyzer), 2);
    assert_eq!(rust_server_phase(&manager), LspServerPhase::Ready);
}

#[test]
fn status_snapshot_is_pure_and_does_not_start_servers() {
    let workspace = rust_workspace();
    let manager =
        LspManager::for_test(workspace.path().to_path_buf(), LspManagerOptions::default());

    let first = manager.status_snapshot();
    let second = manager.status_snapshot();

    assert_eq!(first, second);
    assert_eq!(manager.start_attempts(ServerKind::RustAnalyzer), 0);
    assert_eq!(rust_server_phase(&manager), LspServerPhase::NotStarted);
}

fn rust_workspace() -> TempDir {
    let workspace = tempfile::tempdir().expect("workspace");
    fs::create_dir(workspace.path().join("src")).expect("src directory");
    fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .expect("Cargo.toml");
    fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n").expect("source file");
    workspace
}

fn write_script(workspace: &Path, name: &str, contents: &str) -> PathBuf {
    let path = workspace.join(name);
    fs::write(&path, contents).expect("script");
    let mut permissions = fs::metadata(&path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("script permissions");
    path
}

fn successful_server_script(delay: Duration) -> String {
    format!(
        "#!/bin/sh\nIFS= read -r first_header || exit 9\nprintf 'Content-Length: {}\\r\\n\\r\\n%s' '{}'\n/bin/sleep {}\nprintf 'Content-Length: {}\\r\\n\\r\\n%s' '{}'\nprintf '%s\\n' \"$first_header\" > \"$1\"\n/bin/cat >> \"$1\"\n",
        SERVER_CONFIGURATION_REQUEST.len(),
        SERVER_CONFIGURATION_REQUEST,
        delay.as_secs_f64(),
        INITIALIZE_RESPONSE.len(),
        INITIALIZE_RESPONSE,
    )
}

fn cancel_then_success_server_script() -> String {
    let successful = successful_server_script(Duration::ZERO);
    let successful_body = successful
        .strip_prefix("#!/bin/sh\n")
        .expect("successful script header");
    format!(
        "#!/bin/sh\nif [ ! -f \"$2\" ]; then\n  /usr/bin/touch \"$2\"\n  IFS= read -r first_header || exit 9\n  /bin/sleep 5\n  exit 0\nfi\n{successful_body}"
    )
}

async fn wait_for_transcript(path: &Path, marker: &str) -> String {
    for _ in 0..40 {
        if let Ok(contents) = fs::read_to_string(path)
            && contents.contains(marker)
        {
            return contents;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    fs::read_to_string(path).expect("LSP transcript")
}

async fn wait_for_path(path: &Path) {
    for _ in 0..200 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {}", path.display());
}

fn rust_server_phase(manager: &LspManager) -> LspServerPhase {
    manager
        .status_snapshot()
        .servers
        .into_iter()
        .find(|server| server.name == "rust-analyzer")
        .expect("rust-analyzer status")
        .phase
}
