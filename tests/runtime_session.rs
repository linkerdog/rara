use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use rara::{
    AgentEvent, AgentOutputMode, ContentBlock, LlmBackend, LlmResponse, LlmStreamEvent,
    LlmTurnMetadata, Message, RaraConfig, RuntimeEvent, RuntimeHost, RuntimeSessionBuilder,
    RuntimeSessionError, RuntimeSessionPhase, SessionEvent, TokenUsage, Tool, ToolCallContext,
    ToolError, ToolEvent, ToolManager, ToolProgressEvent,
};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::sync::{Barrier, Notify};
use tokio::time::{Duration, timeout};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn assert_path_tree_excludes(root: &std::path::Path, needle: &str) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        if path != root {
            assert!(
                !path.to_string_lossy().contains(needle),
                "unexpected persisted host artifact: {}",
                path.display()
            );
        }
        if path.is_dir() {
            pending.extend(
                std::fs::read_dir(&path)
                    .expect("read state directory")
                    .map(|entry| entry.expect("state directory entry").path()),
            );
        }
    }
}

struct ToolThenTextBackend {
    calls: AtomicUsize,
    observed_tool_names: Arc<Mutex<Vec<Vec<String>>>>,
}

#[tokio::test]
async fn host_builder_requires_an_explicit_state_root() {
    let temp = tempdir().expect("tempdir");
    let error = RuntimeSessionBuilder::for_host(
        RaraConfig::default(),
        temp.path(),
        Arc::new(FailingBackend),
        ToolManager::new(),
    )
    .build()
    .await
    .err()
    .expect("missing host state root");

    assert!(error.to_string().contains("explicit state root"));
}

impl ToolThenTextBackend {
    fn new(observed_tool_names: Arc<Mutex<Vec<Vec<String>>>>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            observed_tool_names,
        }
    }

    fn record_tools(&self, tools: &[Value]) {
        self.observed_tool_names
            .lock()
            .expect("observed tool names lock")
            .push(
                tools
                    .iter()
                    .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect(),
            );
    }

    fn response(&self) -> LlmResponse {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => LlmResponse {
                content: vec![
                    ContentBlock::ToolUse {
                        id: "provider-call-1".to_string(),
                        name: "host_echo".to_string(),
                        input: json!({"value": "hello"}),
                    },
                    ContentBlock::ToolUse {
                        id: "provider-call-2".to_string(),
                        name: "host_echo".to_string(),
                        input: json!({"value": "again"}),
                    },
                ],
                stop_reason: Some("tool_use".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: 4,
                    output_tokens: 2,
                    ..TokenUsage::default()
                }),
            },
            _ => LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "host tool completed".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: 6,
                    output_tokens: 3,
                    ..TokenUsage::default()
                }),
            },
        }
    }
}

#[async_trait]
impl LlmBackend for ToolThenTextBackend {
    fn model_label(&self) -> Option<String> {
        Some("host-test".to_string())
    }

    async fn ask(&self, _messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        self.record_tools(tools);
        Ok(self.response())
    }

    async fn ask_streaming_with_context(
        &self,
        _messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        metadata.ensure_not_cancelled()?;
        self.record_tools(tools);
        on_event(LlmStreamEvent::ReasoningDelta(
            "checking host tool".to_string(),
        ));
        on_event(LlmStreamEvent::TextDelta("working".to_string()));
        Ok(self.response())
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservedToolContext {
    session_id: Option<String>,
    turn_id: Option<String>,
    call_id: Option<String>,
    workspace_root: Option<String>,
}

struct HostEchoTool {
    observed: Arc<Mutex<Vec<ObservedToolContext>>>,
}

#[async_trait]
impl Tool for HostEchoTool {
    fn name(&self) -> &str {
        "host_echo"
    }

    fn description(&self) -> &str {
        "Echo a host-owned value"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"]
        })
    }

    async fn call(&self, input: Value) -> std::result::Result<Value, ToolError> {
        Ok(input)
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(ToolProgressEvent) + Send),
    ) -> std::result::Result<Value, ToolError> {
        self.observed
            .lock()
            .expect("observed tool context lock")
            .push(ObservedToolContext {
                session_id: context.session_id().map(str::to_string),
                turn_id: context.turn_id().map(str::to_string),
                call_id: context.call_id().map(str::to_string),
                workspace_root: context
                    .workspace_root()
                    .map(|path| path.display().to_string()),
            });
        Ok(input)
    }
}

#[tokio::test]
async fn host_builder_preserves_turn_order_and_provider_tool_identity() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_tool_names = Arc::new(Mutex::new(Vec::new()));
    let state_root = temp.path().join("state");
    let initial_transcript = vec![Message {
        role: "assistant".to_string(),
        content: Value::String("prior host turn".to_string()),
    }];
    let mut tools = ToolManager::new();
    tools.register(Box::new(HostEchoTool {
        observed: observed.clone(),
    }));
    let session = RuntimeSessionBuilder::for_host(
        RaraConfig::default(),
        &workspace,
        Arc::new(ToolThenTextBackend::new(observed_tool_names.clone())),
        tools,
    )
    .with_state_root(&state_root)
    .with_session_id("host-session-1")
    .with_transcript(initial_transcript.clone())
    .build()
    .await
    .expect("runtime session");
    let mut subscription = session
        .subscribe_from_snapshot()
        .expect("snapshot subscription");
    let mut raw_events = Vec::new();

    let outcome = session
        .query_with_events("use the host tool", AgentOutputMode::Silent, |event| {
            raw_events.push(event)
        })
        .await
        .expect("query");

    assert_eq!(session.id().as_str(), "host-session-1");
    assert_eq!(outcome.query_report.model_turns.len(), 2);
    assert_eq!(outcome.transcript.first(), initial_transcript.first());
    assert!(raw_events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolUse { call_id, .. } if call_id == "provider-call-1"
    )));
    assert!(raw_events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult { call_id, .. } if call_id == "provider-call-1"
    )));
    assert!(raw_events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult { call_id, .. } if call_id == "provider-call-2"
    )));
    assert!(
        raw_events
            .iter()
            .all(|event| !matches!(event, AgentEvent::MemoryAction { .. }))
    );
    assert!(
        observed_tool_names
            .lock()
            .expect("observed tool names")
            .iter()
            .all(|names| names == &["host_echo".to_string()])
    );

    let mut control_events = Vec::new();
    loop {
        let event = timeout(TEST_TIMEOUT, subscription.events.recv())
            .await
            .expect("control event timeout")
            .expect("control event");
        let terminal = matches!(
            event.event,
            RuntimeEvent::Session(SessionEvent::TurnFinished { .. })
        );
        control_events.push(event);
        if terminal {
            break;
        }
    }
    assert!(
        control_events
            .windows(2)
            .all(|events| events[0].sequence < events[1].sequence)
    );
    let turn_id = control_events
        .first()
        .and_then(|event| event.turn_id.clone())
        .expect("turn id");
    assert!(
        control_events
            .iter()
            .all(|event| event.turn_id.as_deref() == Some(turn_id.as_str()))
    );
    let call_ids = control_events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::Tool(ToolEvent::Use {
                call_id: Some(call_id),
                ..
            }) => Some(call_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(call_ids, vec!["provider-call-1", "provider-call-2"]);
    let terminal_sequence = control_events.last().expect("terminal event").sequence;
    assert!(
        control_events
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeEvent::Assistant(_) | RuntimeEvent::Tool(_)
            ))
            .all(|event| event.sequence < terminal_sequence)
    );

    {
        let contexts = observed.lock().expect("observed contexts");
        assert_eq!(contexts.len(), 2);
        assert!(contexts.iter().all(|context| {
            context.session_id.as_deref() == Some(session.id().as_str())
                && context.turn_id.as_deref() == Some(turn_id.as_str())
                && context.workspace_root.as_deref() == Some(workspace.to_string_lossy().as_ref())
        }));
        assert_eq!(
            contexts
                .iter()
                .filter_map(|context| context.call_id.as_deref())
                .collect::<Vec<_>>(),
            vec!["provider-call-1", "provider-call-2"]
        );
    }
    assert_eq!(
        session.transcript().await.expect("transcript"),
        outcome.transcript
    );
    let replacement = vec![Message {
        role: "user".to_string(),
        content: Value::String("rehydrated".to_string()),
    }];
    session
        .replace_transcript(replacement.clone())
        .await
        .expect("replace transcript");
    assert_eq!(
        session.transcript().await.expect("replaced transcript"),
        replacement
    );
    assert_path_tree_excludes(&state_root, "host-session-1");
    let (first_shutdown, second_shutdown) = tokio::join!(session.shutdown(), session.shutdown());
    first_shutdown.expect("first concurrent shutdown");
    second_shutdown.expect("second concurrent shutdown");
    session.shutdown().await.expect("idempotent shutdown");
    assert!(matches!(
        timeout(TEST_TIMEOUT, subscription.events.recv())
            .await
            .expect("closed event stream timeout")
            .expect_err("closed event stream"),
        RuntimeSessionError::Closed
    ));
}

struct CancellableBackend {
    started: Arc<Notify>,
}

#[async_trait]
impl LlmBackend for CancellableBackend {
    async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        Err(anyhow!("streaming path required"))
    }

    async fn ask_streaming_with_context(
        &self,
        _messages: &[Message],
        _tools: &[Value],
        metadata: LlmTurnMetadata,
        _on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.started.notify_one();
        loop {
            metadata.ensure_not_cancelled()?;
            tokio::task::yield_now().await;
        }
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".to_string())
    }
}

#[tokio::test]
async fn same_session_rejects_parallel_turn_and_cancels_without_agent_lock() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let started = Arc::new(Notify::new());
    let session = RuntimeSessionBuilder::for_host(
        RaraConfig::default(),
        &workspace,
        Arc::new(CancellableBackend {
            started: started.clone(),
        }),
        ToolManager::new(),
    )
    .with_state_root(temp.path().join("state"))
    .build()
    .await
    .expect("runtime session");
    let snapshots = session.subscribe_snapshots();

    let turn = session
        .submit("block", AgentOutputMode::Silent)
        .await
        .expect("first turn");
    let turn_id = turn.id().clone();
    timeout(TEST_TIMEOUT, started.notified())
        .await
        .expect("backend started");
    let busy = session
        .submit("second", AgentOutputMode::Silent)
        .await
        .expect_err("parallel turn must be rejected");
    assert!(matches!(
        busy,
        RuntimeSessionError::Busy { active_turn } if active_turn == turn_id
    ));
    assert!(matches!(
        session
            .transcript()
            .await
            .expect_err("running transcript must be rejected"),
        RuntimeSessionError::Busy { active_turn } if active_turn == turn_id
    ));
    assert_eq!(
        session.cancel().await.expect("cancel acknowledgement"),
        turn_id
    );
    let cancelled = timeout(TEST_TIMEOUT, turn.wait())
        .await
        .expect("turn cancellation timeout")
        .expect_err("cancelled turn");
    assert!(matches!(
        cancelled,
        RuntimeSessionError::Cancelled { ref outcome }
            if outcome.turn_id == turn_id && !outcome.transcript.is_empty()
    ));
    assert_eq!(
        cancelled
            .turn_outcome()
            .expect("cancelled turn evidence")
            .turn_id,
        turn_id
    );
    assert!(matches!(
        snapshots.borrow().phase,
        RuntimeSessionPhase::Idle
    ));
    assert!(matches!(
        session.snapshot().phase,
        RuntimeSessionPhase::Idle
    ));
    session.shutdown().await.expect("shutdown");
}

struct FailingBackend;

#[async_trait]
impl LlmBackend for FailingBackend {
    async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        Err(anyhow!("provider failed"))
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".to_string())
    }
}

#[tokio::test]
async fn failed_turn_retains_evidence_and_publishes_a_terminal_failure() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let session = RuntimeSessionBuilder::for_host(
        RaraConfig::default(),
        &workspace,
        Arc::new(FailingBackend),
        ToolManager::new(),
    )
    .with_state_root(temp.path().join("state"))
    .build()
    .await
    .expect("runtime session");
    let mut subscription = session
        .subscribe_from_snapshot()
        .expect("snapshot subscription");

    let error = session
        .query_with_events("fail", AgentOutputMode::Silent, |_| {})
        .await
        .expect_err("failed provider turn");
    assert!(matches!(
        error,
        RuntimeSessionError::Execution {
            ref message,
            ref outcome,
        } if message.contains("provider failed") && !outcome.transcript.is_empty()
    ));

    let mut events = Vec::new();
    loop {
        let event = timeout(TEST_TIMEOUT, subscription.events.recv())
            .await
            .expect("control event timeout")
            .expect("control event");
        let terminal = matches!(
            event.event,
            RuntimeEvent::Session(SessionEvent::TurnFailed { .. })
        );
        events.push(event);
        if terminal {
            break;
        }
    }
    assert!(matches!(
        &events.last().expect("terminal failure").event,
        RuntimeEvent::Session(SessionEvent::TurnFailed { reason })
            if reason.contains("provider failed")
    ));
    session.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn closed_event_stream_drains_published_events_before_closing() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let session = RuntimeSessionBuilder::for_host(
        RaraConfig::default(),
        &workspace,
        Arc::new(FailingBackend),
        ToolManager::new(),
    )
    .with_state_root(temp.path().join("state"))
    .build()
    .await
    .expect("runtime session");
    let mut subscription = session
        .subscribe_from_snapshot()
        .expect("snapshot subscription");

    session
        .query_with_events("fail", AgentOutputMode::Silent, |_| {})
        .await
        .expect_err("failed provider turn");
    session.shutdown().await.expect("shutdown");

    let mut observed_terminal = false;
    loop {
        match timeout(TEST_TIMEOUT, subscription.events.recv())
            .await
            .expect("event stream timeout")
        {
            Ok(event) => {
                observed_terminal |= matches!(
                    event.event,
                    RuntimeEvent::Session(SessionEvent::TurnFailed { .. })
                );
            }
            Err(RuntimeSessionError::Closed) => break,
            Err(error) => panic!("unexpected event stream error: {error}"),
        }
    }
    assert!(observed_terminal);

    let mut closed_subscription = session
        .subscribe_from_snapshot()
        .expect("closed snapshot subscription");
    assert!(matches!(
        closed_subscription.events.recv().await,
        Err(RuntimeSessionError::Closed)
    ));
}

struct BarrierBackend {
    barrier: Arc<Barrier>,
}

#[async_trait]
impl LlmBackend for BarrierBackend {
    async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        self.barrier.wait().await;
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: None,
        })
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".to_string())
    }
}

#[tokio::test]
async fn runtime_host_allows_different_sessions_to_progress_concurrently() {
    let temp = tempdir().expect("tempdir");
    let barrier = Arc::new(Barrier::new(3));
    let mut sessions = Vec::new();
    for index in 0..2 {
        let workspace = temp.path().join(format!("workspace-{index}"));
        std::fs::create_dir_all(&workspace).expect("workspace");
        let session = RuntimeSessionBuilder::for_host(
            RaraConfig::default(),
            &workspace,
            Arc::new(BarrierBackend {
                barrier: barrier.clone(),
            }),
            ToolManager::new(),
        )
        .with_state_root(temp.path().join(format!("state-{index}")))
        .build()
        .await
        .expect("runtime session");
        sessions.push(session);
    }
    let host = RuntimeHost::new();
    for session in &sessions {
        host.insert(session.clone()).await.expect("insert session");
    }
    assert_eq!(host.session_ids().await.len(), 2);
    assert_eq!(
        host.get(sessions[0].id()).await.expect("host lookup").id(),
        sessions[0].id()
    );
    assert!(matches!(
        host.insert(sessions[0].clone())
            .await
            .expect_err("duplicate session"),
        RuntimeSessionError::AlreadyExists(id) if id == sessions[0].id().clone()
    ));

    let first = sessions[0]
        .submit("first", AgentOutputMode::Silent)
        .await
        .expect("first turn");
    let second = sessions[1]
        .submit("second", AgentOutputMode::Silent)
        .await
        .expect("second turn");
    timeout(TEST_TIMEOUT, barrier.wait())
        .await
        .expect("both sessions reached provider boundary");
    timeout(TEST_TIMEOUT, first.wait())
        .await
        .expect("first completion timeout")
        .expect("first completion");
    timeout(TEST_TIMEOUT, second.wait())
        .await
        .expect("second completion timeout")
        .expect("second completion");
    host.shutdown().await.expect("host shutdown");
    assert!(host.session_ids().await.is_empty());
    assert!(
        sessions
            .iter()
            .all(|session| matches!(session.snapshot().phase, RuntimeSessionPhase::Closed))
    );
}
