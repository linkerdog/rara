// Runtime host fixtures use assertions and no ambient providers.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use rara_app_server::runtime_control::{
    PromptSourceControlRequest, PromptSourceLifetime, PromptSourceRegistration,
    RuntimeControllerKind, SourceLayer, SourceScope,
};
use serde_json::Value;
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

use crate::{
    AgentOutputMode, ContentBlock, LlmBackend, LlmResponse, Message, RaraConfig, RuntimeProvenance,
    RuntimeSession, RuntimeSessionBuilder, RuntimeSessionError, ToolManager,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const CONTEXT: &str = "session-a source evidence";

#[derive(Default)]
struct CapturedBackend {
    requests: Mutex<Vec<Vec<Message>>>,
    started: Notify,
    release: Notify,
}

#[async_trait]
impl LlmBackend for CapturedBackend {
    async fn ask(&self, messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        self.requests
            .lock()
            .expect("requests")
            .push(messages.to_vec());
        self.started.notify_one();
        self.release.notified().await;
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "done".into(),
            }],
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        panic!("source fixture must not summarize")
    }
}

async fn session(
    root: &std::path::Path,
    id: &str,
    backend: Arc<CapturedBackend>,
) -> RuntimeSession {
    RuntimeSessionBuilder::for_host(RaraConfig::default(), root, backend, ToolManager::new())
        .with_session_id(id)
        .with_state_root(root.join(format!("state-{id}")))
        .build()
        .await
        .expect("isolated host session")
}

fn registration() -> PromptSourceControlRequest {
    PromptSourceControlRequest::Register(PromptSourceRegistration {
        source_id: "task-context".into(),
        scope: SourceScope::Session,
        layer: SourceLayer::User,
        budget_hint_tokens: Some(64),
        lifetime: PromptSourceLifetime::Turns(1),
        content: CONTEXT.into(),
    })
}

#[tokio::test]
async fn canonical_sources_reach_only_the_target_session_and_expire_after_query() {
    let root = tempfile::tempdir().expect("workspace");
    let backend_a = Arc::new(CapturedBackend::default());
    let backend_b = Arc::new(CapturedBackend::default());
    let session_a = session(root.path(), "session-a", backend_a.clone()).await;
    let session_b = session(root.path(), "session-b", backend_b.clone()).await;
    let provenance = RuntimeProvenance::protocol(
        RuntimeControllerKind::AppServer,
        "stdio-jsonl",
        Some(session_a.id().to_string()),
        Some("task-context".into()),
    );
    assert!(matches!(
        session_b
            .apply_prompt_source(registration(), provenance.clone())
            .await,
        Err(RuntimeSessionError::InvalidSource)
    ));
    session_a
        .apply_prompt_source(registration(), provenance.clone())
        .await
        .expect("register context");
    let first = session_a
        .submit("first query", AgentOutputMode::Silent)
        .await
        .expect("first turn");
    timeout(TEST_TIMEOUT, backend_a.started.notified())
        .await
        .expect("first provider request");
    assert!(
        matches!(session_a.apply_prompt_source(registration(), provenance.clone()).await,
        Err(RuntimeSessionError::Busy { active_turn }) if &active_turn == first.id())
    );
    {
        let requests = backend_a.requests.lock().expect("requests");
        let first = &requests[0];
        assert!(
            !first[0].content.to_string().contains(CONTEXT),
            "stable system prefix"
        );
        assert!(
            first
                .iter()
                .any(|message| message.role == "user"
                    && message.content.to_string().contains(CONTEXT)),
            "registered context delivered through user context"
        );
    }
    backend_a.release.notify_one();
    timeout(TEST_TIMEOUT, first.wait())
        .await
        .expect("first completion")
        .expect("first outcome");
    // Prior transcript content remains historical evidence; inspect a fresh query context.
    session_a
        .replace_transcript(Vec::new())
        .await
        .expect("clear transcript for expiry observation");
    let second = session_a
        .submit("second query", AgentOutputMode::Silent)
        .await
        .expect("second turn");
    timeout(TEST_TIMEOUT, backend_a.started.notified())
        .await
        .expect("second provider request");
    assert!(
        backend_a.requests.lock().expect("requests")[1]
            .iter()
            .all(|message| !message.content.to_string().contains(CONTEXT))
    );
    backend_a.release.notify_one();
    timeout(TEST_TIMEOUT, second.wait())
        .await
        .expect("second completion")
        .expect("second outcome");
    let other = session_b
        .submit("other session", AgentOutputMode::Silent)
        .await
        .expect("other turn");
    timeout(TEST_TIMEOUT, backend_b.started.notified())
        .await
        .expect("other provider request");
    assert!(
        backend_b.requests.lock().expect("requests")[0]
            .iter()
            .all(|message| !message.content.to_string().contains(CONTEXT))
    );
    backend_b.release.notify_one();
    timeout(TEST_TIMEOUT, other.wait())
        .await
        .expect("other completion")
        .expect("other outcome");
    session_a.shutdown().await.expect("first shutdown");
    session_b.shutdown().await.expect("second shutdown");
    assert!(matches!(
        session_a
            .apply_prompt_source(registration(), provenance)
            .await,
        Err(RuntimeSessionError::Closed)
    ));
}
