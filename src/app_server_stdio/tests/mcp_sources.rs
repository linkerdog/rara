use rara_app_server::runtime_control::McpSourceControlRequest;
use rara_app_server::stdio_protocol::RejectionCode;

use super::*;
use crate::runtime_control::McpEvent;

#[tokio::test]
async fn source_controls_keep_receipt_identity_and_session_owned_events() {
    let root = tempfile::tempdir().unwrap();
    let mut harness = Harness::start(root.path(), Arc::new(Backend::default())).await;
    let session = harness.create().await;
    let request = McpSourceControlRequest::Register(
        crate::runtime_session::mcp_source_registration_fixture(root.path(), "wire-source"),
    );
    let frame = harness.control(
        "source-register",
        Some(&session),
        RuntimeControlRequest::McpSource(request),
    );
    harness.send(&frame).await;
    let first = harness.ack("source-register").await;
    assert!(matches!(first.result, RequestResult::Accepted { .. }));
    let pid = std::fs::read_to_string(root.path().join("wire-source.jsonl.pid")).unwrap();
    harness.send(&frame).await;
    assert_eq!(harness.ack("source-register").await, first);
    assert_eq!(
        std::fs::read_to_string(root.path().join("wire-source.jsonl.pid")).unwrap(),
        pid
    );
    let foreign = harness.control(
        "foreign-source",
        Some("foreign"),
        RuntimeControlRequest::McpSource(McpSourceControlRequest::Unregister {
            source_id: "wire-source".into(),
        }),
    );
    harness.send(&foreign).await;
    assert!(matches!(
        harness.ack("foreign-source").await.result,
        RequestResult::Rejected {
            code: RejectionCode::UnknownSession,
            ..
        }
    ));
    let remove = harness.control(
        "source-remove",
        Some(&session),
        RuntimeControlRequest::McpSource(McpSourceControlRequest::Unregister {
            source_id: "wire-source".into(),
        }),
    );
    harness.send(&remove).await;
    assert!(matches!(
        harness.ack("source-remove").await.result,
        RequestResult::Accepted { .. }
    ));
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    let session_handle = harness
        .host
        .get(&RuntimeSessionId::new(&session))
        .await
        .unwrap();
    let events = session_handle.replay_events(0).unwrap();
    let registered: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event.event,
                RuntimeEvent::Mcp(McpEvent::SourceRegistered { .. })
            )
        })
        .collect();
    assert_eq!(registered.len(), 1);
    assert_eq!(
        registered[0].provenance.session_id.as_deref(),
        Some(session.as_str())
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("SOURCE_SCOPE")
    );
    harness.shutdown().await;
}
