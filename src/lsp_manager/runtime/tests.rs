use std::time::Duration;

use tokio::sync::watch;

use super::{LspFailure, LspFailureKind, ServerKind, resolve_startup_failure};

#[tokio::test]
async fn startup_write_failure_waits_for_exit_code_and_stderr() {
    let failure = LspFailure::new(LspFailureKind::ProtocolError, "broken stdin pipe", true);
    let exit = LspFailure::new(LspFailureKind::ServerExited, "server exited", true)
        .for_server(ServerKind::RustAnalyzer)
        .with_process_status(Some(17), None, "synthetic crash\n".into());
    let (sender, receiver) = watch::channel(None);
    let expected = exit.clone();
    let supervisor = tokio::spawn(async move {
        tokio::task::yield_now().await;
        sender.send_replace(Some(exit));
    });

    assert_eq!(resolve_startup_failure(failure, receiver).await, expected);
    supervisor.await.expect("supervisor completes");
}

#[tokio::test]
async fn live_server_keeps_the_protocol_failure_within_a_bounded_wait() {
    let failure = LspFailure::new(LspFailureKind::ProtocolError, "invalid response", true);
    let (_sender, receiver) = watch::channel(None);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        resolve_startup_failure(failure.clone(), receiver),
    )
    .await
    .expect("live servers must not stall startup failure reporting");
    assert_eq!(result, failure);
}

#[tokio::test]
async fn initialization_timeout_is_not_reclassified_as_exit() {
    let failure = LspFailure::new(LspFailureKind::InitializeTimeout, "deadline elapsed", true);
    let exit = LspFailure::new(LspFailureKind::ServerExited, "server exited", true);
    let (_sender, receiver) = watch::channel(Some(exit));
    assert_eq!(
        resolve_startup_failure(failure.clone(), receiver).await,
        failure
    );
}
