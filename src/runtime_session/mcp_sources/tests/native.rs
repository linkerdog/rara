use std::sync::atomic::AtomicBool;

use anyhow::Result;
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

use super::*;
use crate::runtime_control::ToolEvent;
use crate::runtime_session::{RuntimeSession, RuntimeSessionBuilder, RuntimeSessionProfile};
use crate::{
    AgentOutputMode, ContentBlock, LlmBackend, LlmResponse, Message, RaraConfig, RuntimeEvent,
    RuntimeProvenance,
};

#[derive(Default)]
struct Backend {
    invoke: AtomicBool,
    schemas: std::sync::Mutex<Vec<Vec<Value>>>,
    started: Notify,
    gate: Option<Notify>,
}

#[async_trait]
impl LlmBackend for Backend {
    async fn ask(&self, _messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        self.schemas.lock().unwrap().push(tools.to_vec());
        self.started.notify_one();
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        let content = if self.invoke.swap(false, Ordering::SeqCst) {
            vec![ContentBlock::ToolUse {
                id: "owned-call".into(),
                name: tools[0]["name"].as_str().unwrap().into(),
                input: json!({"value": 9}),
            }]
        } else {
            vec![ContentBlock::Text {
                text: "done".into(),
            }]
        };
        Ok(LlmResponse {
            content,
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        anyhow::bail!("source fixture must not summarize")
    }
}

fn builder(root: &Path, id: &str, backend: Arc<Backend>) -> RuntimeSessionBuilder {
    RuntimeSessionBuilder::for_host(RaraConfig::default(), root, backend, ToolManager::new())
        .with_state_root(root.join("state"))
        .with_session_id(id)
}

fn origin(session: &RuntimeSession) -> RuntimeProvenance {
    RuntimeProvenance::runtime(Some(session.id().to_string()))
}

async fn query(session: &RuntimeSession) {
    timeout(
        Duration::from_secs(10),
        session
            .submit("perform the scoped operation", AgentOutputMode::Silent)
            .await
            .unwrap()
            .wait(),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn registered_tools_reach_native_calls_and_stay_out_of_foreign_sessions() {
    let root_a = tempfile::tempdir().unwrap();
    let root_b = tempfile::tempdir().unwrap();
    let backend_a = Arc::new(Backend::default());
    let backend_b = Arc::new(Backend::default());
    let a = builder(root_a.path(), "native-a", backend_a.clone())
        .with_controlled_mcp_sources()
        .build()
        .await
        .unwrap();
    let b = builder(root_b.path(), "native-b", backend_b.clone())
        .with_controlled_mcp_sources()
        .build()
        .await
        .unwrap();
    let request = McpSourceControlRequest::Register(registration(root_a.path(), "alpha"));
    assert!(matches!(
        b.apply_mcp_source(request.clone(), origin(&a)).await,
        Err(RuntimeSessionError::InvalidSource)
    ));
    a.apply_mcp_source(request, origin(&a)).await.unwrap();
    backend_a.invoke.store(true, Ordering::SeqCst);
    query(&a).await;
    query(&b).await;
    assert!(backend_b.schemas.lock().unwrap().iter().all(Vec::is_empty));
    assert_eq!(backend_a.schemas.lock().unwrap()[0].len(), 1);
    let events = a.replay_events(0).unwrap();
    let result = events
        .iter()
        .find_map(|event| match &event.event {
            RuntimeEvent::Tool(ToolEvent::Result {
                call_id,
                content,
                is_error,
                ..
            }) if call_id.as_deref() == Some("owned-call") => {
                assert!(!is_error);
                assert_eq!(
                    event.provenance.session_id.as_deref(),
                    Some(a.id().as_str())
                );
                Some(content.clone())
            }
            _ => None,
        })
        .expect("native tool result");
    assert!(
        result.contains("alpha"),
        "scoped result must reach the native transcript"
    );
    assert_eq!(
        std::fs::read_to_string(root_a.path().join("alpha.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    a.apply_mcp_source(
        McpSourceControlRequest::Unregister {
            source_id: "alpha".into(),
        },
        origin(&a),
    )
    .await
    .unwrap();
    query(&a).await;
    assert!(backend_a.schemas.lock().unwrap().last().unwrap().is_empty());
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

#[tokio::test]
async fn busy_session_refuses_registration_before_spawning() {
    let root = tempfile::tempdir().unwrap();
    let backend = Arc::new(Backend {
        gate: Some(Notify::new()),
        ..Backend::default()
    });
    let session = builder(root.path(), "busy", backend.clone())
        .with_controlled_mcp_sources()
        .build()
        .await
        .unwrap();
    let turn = session
        .submit("hold", AgentOutputMode::Silent)
        .await
        .unwrap();
    timeout(Duration::from_secs(10), backend.started.notified())
        .await
        .unwrap();
    assert!(matches!(
        session
            .apply_mcp_source(
                McpSourceControlRequest::Register(registration(root.path(), "alpha")),
                origin(&session)
            )
            .await,
        Err(RuntimeSessionError::Busy { .. })
    ));
    assert!(!root.path().join("alpha.jsonl.pid").exists());
    backend.gate.as_ref().unwrap().notify_one();
    timeout(Duration::from_secs(10), turn.wait())
        .await
        .unwrap()
        .unwrap();
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn exact_host_tools_and_frozen_profiles_are_not_widened_implicitly() {
    let root = tempfile::tempdir().unwrap();
    let backend = Arc::new(Backend::default());
    let session = builder(root.path(), "disabled", backend.clone())
        .build()
        .await
        .unwrap();
    assert!(matches!(
        session
            .apply_mcp_source(
                McpSourceControlRequest::Register(registration(root.path(), "alpha")),
                origin(&session)
            )
            .await,
        Err(RuntimeSessionError::UnsupportedSource)
    ));
    assert!(!root.path().join("alpha.jsonl.pid").exists());
    session.shutdown().await.unwrap();
    assert!(
        builder(root.path(), "frozen", backend.clone())
            .with_controlled_mcp_sources()
            .with_profile(RuntimeSessionProfile::HeadlessCodingV1)
            .build()
            .await
            .is_err()
    );
    assert!(
        builder(root.path(), "frozen-reverse", backend)
            .with_profile(RuntimeSessionProfile::HeadlessCodingV1)
            .with_controlled_mcp_sources()
            .build()
            .await
            .is_err()
    );
    assert!(
        builder(root.path(), "stable", Arc::new(Backend::default()))
            .with_controlled_mcp_sources()
            .with_cache_experiment(crate::agent::CacheExperimentOptions {
                tool_schemas: crate::agent::ToolSchemaPolicy::SessionStable,
                ..Default::default()
            })
            .build()
            .await
            .is_err()
    );
}

#[tokio::test]
async fn uncertain_registration_prevents_replacement_turns_and_successful_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let session = builder(root.path(), "uncertain", Arc::new(Backend::default()))
        .with_controlled_mcp_sources()
        .build()
        .await
        .unwrap();
    let mut request = registration(root.path(), "alpha");
    request.command = root
        .path()
        .join("missing-executable")
        .to_string_lossy()
        .into_owned();
    assert!(matches!(
        session
            .apply_mcp_source(McpSourceControlRequest::Register(request), origin(&session))
            .await,
        Err(RuntimeSessionError::SourceUnavailable)
    ));
    assert!(matches!(
        session.submit("replacement", AgentOutputMode::Silent).await,
        Err(RuntimeSessionError::SourceUnavailable)
    ));
    assert!(session.shutdown().await.is_err());
}
