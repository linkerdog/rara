//! Runtime-owned session capture for local and hosted Nowledge Mem.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::json;

use crate::config::NowledgeMemPluginConfig;
use crate::runtime_control::{RuntimeEvent, WarningEvent};
use crate::runtime_event_bus::RuntimeEventBus;

const SOURCE_APP: &str = "rara";
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemorySessionMessage {
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) external_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MemorySessionSnapshot {
    pub(crate) session_id: String,
    pub(crate) workspace: String,
    pub(crate) space_id: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) host_agent_id: Option<String>,
    pub(crate) messages: Vec<MemorySessionMessage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemorySyncReason {
    TurnIdle,
    Compaction,
    Shutdown,
}

impl MemorySyncReason {
    fn label(self) -> &'static str {
        match self {
            Self::TurnIdle => "turn_idle",
            Self::Compaction => "compaction",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MemoryAppendRequest {
    pub(crate) thread_id: String,
    pub(crate) messages: Vec<MemorySessionMessage>,
    pub(crate) idempotency_key: String,
    pub(crate) workspace: String,
    pub(crate) space_id: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) host_agent_id: Option<String>,
    pub(crate) reason: MemorySyncReason,
}

/// Transport boundary for runtime-owned session capture.
#[async_trait]
pub(crate) trait MemorySessionSink: Send + Sync {
    async fn append(&self, request: MemoryAppendRequest) -> Result<()>;
}

#[derive(Default)]
struct MemorySyncState {
    sent_external_ids: BTreeSet<String>,
    last_signature: Option<String>,
}

/// Coordinates best-effort session capture without blocking agent execution.
pub(crate) struct MemoryLifecycleCoordinator {
    sink: Arc<dyn MemorySessionSink>,
    state: Mutex<MemorySyncState>,
    event_bus: Arc<RuntimeEventBus>,
}

impl MemoryLifecycleCoordinator {
    pub(crate) fn from_config(
        config: &NowledgeMemPluginConfig,
        event_bus: Arc<RuntimeEventBus>,
    ) -> Self {
        let sink: Arc<dyn MemorySessionSink> = if config.enabled {
            Arc::new(HttpMemorySessionSink::new(config))
        } else {
            Arc::new(DisabledMemorySessionSink)
        };
        Self::new(sink, event_bus)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        sink: Arc<dyn MemorySessionSink>,
        event_bus: Arc<RuntimeEventBus>,
    ) -> Self {
        Self::new(sink, event_bus)
    }

    fn new(sink: Arc<dyn MemorySessionSink>, event_bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            sink,
            state: Mutex::new(MemorySyncState::default()),
            event_bus,
        }
    }

    pub(crate) async fn capture(
        &self,
        snapshot: MemorySessionSnapshot,
        reason: MemorySyncReason,
    ) -> bool {
        let Some(request) = self.prepare_request(&snapshot, reason) else {
            return true;
        };
        let signature = request
            .messages
            .last()
            .map(|message| message.external_id.clone())
            .unwrap_or_default();
        let result = tokio::time::timeout(CAPTURE_TIMEOUT, self.sink.append(request)).await;
        match result {
            Ok(Ok(())) => {
                let mut state = self.state.lock().expect("memory sync state lock");
                state.sent_external_ids.extend(
                    snapshot
                        .messages
                        .iter()
                        .map(|message| message.external_id.clone()),
                );
                state.last_signature = Some(signature);
                true
            }
            Ok(Err(err)) => {
                self.warn(format!("Nowledge Mem session capture failed: {err:#}"));
                false
            }
            Err(_) => {
                self.warn("Nowledge Mem session capture timed out".to_string());
                false
            }
        }
    }

    pub(crate) async fn drain(&self, snapshot: MemorySessionSnapshot) -> bool {
        tokio::time::timeout(
            SHUTDOWN_TIMEOUT,
            self.capture(snapshot, MemorySyncReason::Shutdown),
        )
        .await
        .unwrap_or_else(|_| {
            self.warn("Nowledge Mem shutdown capture timed out".to_string());
            false
        })
    }

    fn prepare_request(
        &self,
        snapshot: &MemorySessionSnapshot,
        reason: MemorySyncReason,
    ) -> Option<MemoryAppendRequest> {
        if snapshot.session_id.is_empty() || snapshot.messages.is_empty() {
            return None;
        }
        let state = self.state.lock().expect("memory sync state lock");
        let messages: Vec<_> = snapshot
            .messages
            .iter()
            .filter(|message| !state.sent_external_ids.contains(&message.external_id))
            .cloned()
            .collect();
        if messages.is_empty() {
            return None;
        }
        let last_id = messages
            .last()
            .map(|message| message.external_id.as_str())
            .unwrap_or_default();
        let signature = format!("{}:{last_id}", messages.len());
        if state.last_signature.as_deref() == Some(signature.as_str()) {
            return None;
        }
        drop(state);

        let thread_id = format!("{SOURCE_APP}-{}", snapshot.session_id);
        Some(MemoryAppendRequest {
            idempotency_key: format!("{thread_id}:{}:{last_id}", reason.label()),
            thread_id,
            messages,
            workspace: snapshot.workspace.clone(),
            space_id: snapshot.space_id.clone(),
            agent_id: snapshot.agent_id.clone(),
            host_agent_id: snapshot.host_agent_id.clone(),
            reason,
        })
    }

    fn warn(&self, message: String) {
        self.event_bus
            .publish_control(RuntimeEvent::Warning(WarningEvent::RuntimeWarning {
                message,
            }));
    }
}

struct DisabledMemorySessionSink;

#[async_trait]
impl MemorySessionSink for DisabledMemorySessionSink {
    async fn append(&self, _request: MemoryAppendRequest) -> Result<()> {
        Ok(())
    }
}

struct HttpMemorySessionSink {
    client: reqwest::Client,
    base_url: String,
    headers: Vec<(String, String)>,
}

impl HttpMemorySessionSink {
    fn new(config: &NowledgeMemPluginConfig) -> Self {
        let mut headers = Vec::new();
        let api_key = config
            .api_key()
            .map(str::to_string)
            .or_else(|| std::env::var(&config.api_key_env_var).ok());
        if let Some(api_key) = api_key {
            headers.push(("Authorization".to_string(), format!("Bearer {api_key}")));
            headers.push(("X-NMEM-API-Key".to_string(), api_key));
        }
        if let Some(space) = config
            .space_id_env_var
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
        {
            headers.push(("X-Nmem-Space-Id".to_string(), space));
        }
        headers.extend(config.http_headers.clone());
        Self {
            client: reqwest::Client::new(),
            base_url: config.api_url(),
            headers,
        }
    }
}

#[async_trait]
impl MemorySessionSink for HttpMemorySessionSink {
    async fn append(&self, request: MemoryAppendRequest) -> Result<()> {
        let metadata = json!({
            "source_app": SOURCE_APP,
            "workspace": &request.workspace,
            "space_id": &request.space_id,
            "agent_id": &request.agent_id,
            "host_agent_id": &request.host_agent_id,
            "sync_reason": request.reason.label(),
        });
        let messages = request
            .messages
            .into_iter()
            .map(|message| {
                json!({
                    "role": message.role,
                    "content": message.content,
                    "metadata": {
                        "external_id": message.external_id,
                        "source_app": SOURCE_APP,
                    },
                })
            })
            .collect::<Vec<_>>();
        let base = self.base_url.trim_end_matches('/');
        let create_body = json!({
            "thread_id": &request.thread_id,
            "title": "RARA Session",
            "messages": messages,
            "source": SOURCE_APP,
            "project": &request.workspace,
            "workspace": &request.workspace,
            "metadata": metadata,
            "space_id": &request.space_id,
            "idempotency_key": &request.idempotency_key,
        });
        let create_response = self
            .send_json(&format!("{base}/threads"), &create_body)
            .await?;
        if create_response.is_success() {
            return Ok(());
        }

        let append_body = json!({
            "messages": create_body["messages"],
            "deduplicate": true,
            "idempotency_key": request.idempotency_key,
            "metadata": create_body["metadata"],
            "source": SOURCE_APP,
            "space_id": request.space_id,
        });
        let endpoint = format!(
            "{base}/threads/{}/append",
            urlencoding::encode(&request.thread_id)
        );
        let response = self.send_json(&endpoint, &append_body).await?;
        if response.is_success() || response == StatusCode::CONFLICT {
            return Ok(());
        }
        anyhow::bail!("memory append returned {response}")
    }
}

impl HttpMemorySessionSink {
    async fn send_json(&self, endpoint: &str, body: &serde_json::Value) -> Result<StatusCode> {
        let mut builder = self.client.post(endpoint).json(body);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        Ok(builder
            .send()
            .await
            .context("send memory session request")?
            .status())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeSink {
        requests: Mutex<Vec<MemoryAppendRequest>>,
        fail: Mutex<bool>,
    }

    #[async_trait]
    impl MemorySessionSink for FakeSink {
        async fn append(&self, request: MemoryAppendRequest) -> Result<()> {
            if *self.fail.lock().expect("fail lock") {
                anyhow::bail!("fake sink failure")
            }
            self.requests.lock().expect("request lock").push(request);
            Ok(())
        }
    }

    fn snapshot(count: usize) -> MemorySessionSnapshot {
        MemorySessionSnapshot {
            session_id: "session-1".to_string(),
            workspace: "/workspace".to_string(),
            space_id: Some("space-1".to_string()),
            agent_id: Some("agent-1".to_string()),
            host_agent_id: Some("host-1".to_string()),
            messages: (0..count)
                .map(|index| MemorySessionMessage {
                    role: if index % 2 == 0 {
                        "user".to_string()
                    } else {
                        "assistant".to_string()
                    },
                    content: format!("message-{index}"),
                    external_id: format!("rara-msg-session-1-{index}"),
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn capture_uses_stable_thread_and_deduplicates_messages() {
        let sink = Arc::new(FakeSink::default());
        let coordinator = MemoryLifecycleCoordinator::new_for_test(
            sink.clone(),
            Arc::new(RuntimeEventBus::new(8)),
        );

        assert!(
            coordinator
                .capture(snapshot(2), MemorySyncReason::TurnIdle)
                .await
        );
        assert!(
            coordinator
                .capture(snapshot(2), MemorySyncReason::TurnIdle)
                .await
        );
        assert!(
            coordinator
                .capture(snapshot(3), MemorySyncReason::Compaction)
                .await
        );

        let requests = sink.requests.lock().expect("request lock");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].thread_id, "rara-session-1");
        assert_eq!(requests[0].workspace, "/workspace");
        assert_eq!(requests[0].space_id.as_deref(), Some("space-1"));
        assert_eq!(requests[0].agent_id.as_deref(), Some("agent-1"));
        assert_eq!(requests[0].host_agent_id.as_deref(), Some("host-1"));
        assert_eq!(requests[0].reason, MemorySyncReason::TurnIdle);
        assert_eq!(
            requests[0].idempotency_key,
            "rara-session-1:turn_idle:rara-msg-session-1-1"
        );
        assert_eq!(requests[1].messages.len(), 1);
        assert_eq!(requests[1].messages[0].external_id, "rara-msg-session-1-2");
    }

    #[tokio::test]
    async fn failed_capture_reports_warning_and_retries() {
        let sink = Arc::new(FakeSink::default());
        *sink.fail.lock().expect("fail lock") = true;
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut events = bus.subscribe_control();
        let coordinator = MemoryLifecycleCoordinator::new_for_test(sink.clone(), bus);

        assert!(
            !coordinator
                .capture(snapshot(1), MemorySyncReason::TurnIdle)
                .await
        );
        assert!(matches!(
            events.try_recv().expect("warning event").event,
            RuntimeEvent::Warning(WarningEvent::RuntimeWarning { .. })
        ));
        *sink.fail.lock().expect("fail lock") = false;
        assert!(
            coordinator
                .capture(snapshot(1), MemorySyncReason::TurnIdle)
                .await
        );
        assert_eq!(sink.requests.lock().expect("request lock").len(), 1);
    }
}
