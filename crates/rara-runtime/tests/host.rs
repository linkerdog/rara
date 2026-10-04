// Public consumer fixture; copied unchanged into a fresh external Git consumer.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use rara_runtime::{
    AssistantEvent, ContentBlock, LlmBackend, LlmResponse, LlmStreamEvent, LlmTurnMetadata,
    Message, RuntimeEvent, RuntimeSession, RuntimeSessionBuilder, RuntimeSessionError,
    RuntimeSessionPhase, SessionEvent, Tool, ToolCallContext, ToolError, ToolEvent, ToolManager,
    ToolProgressEvent,
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Backend {
    responses: Mutex<VecDeque<LlmResponse>>,
    requests: Mutex<Vec<Vec<Message>>>,
    gate: Option<Arc<Gate>>,
}

#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
    cancellation: Mutex<Option<LlmTurnMetadata>>,
}

impl Backend {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            gate: None,
        }
    }
}

#[async_trait]
impl LlmBackend for Backend {
    async fn ask(&self, _: &[Message], _: &[Value]) -> Result<LlmResponse> {
        Err(anyhow!("context-aware streaming required"))
    }
    async fn summarize(&self, _: &[Message], _: &str) -> Result<String> {
        Err(anyhow!("host fixture must not summarize"))
    }
    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        _: &[Value],
        metadata: LlmTurnMetadata,
        report: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        metadata.ensure_not_cancelled()?;
        self.requests.lock().unwrap().push(messages.to_vec());
        report(LlmStreamEvent::TextDelta("streamed ".into()));
        if let Some(gate) = &self.gate {
            *gate.cancellation.lock().unwrap() = Some(metadata);
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        report(LlmStreamEvent::TextDelta("tail".into()));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow!("provider failed"))
    }
}

fn response(content: Vec<ContentBlock>) -> LlmResponse {
    LlmResponse {
        content,
        usage: None,
        stop_reason: Some("end_turn".into()),
    }
}

fn text_response() -> LlmResponse {
    response(vec![ContentBlock::Text {
        text: "final response".into(),
    }])
}

fn calls_response(ids: &[&str]) -> LlmResponse {
    response(
        ids.iter()
            .map(|id| ContentBlock::ToolUse {
                id: (*id).into(),
                name: "host_echo".into(),
                input: json!({"value": id, "session_id": "forged", "call_id": "forged"}),
            })
            .collect(),
    )
}

struct EchoTool {
    calls: Arc<Mutex<Vec<ToolCallContext>>>,
    gate: Option<Arc<Gate>>,
    returned: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "host_echo"
    }
    fn description(&self) -> &str {
        "Echo with trusted execution identity"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _: Value) -> Result<Value, ToolError> {
        panic!("trusted context required")
    }
    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _: &mut (dyn FnMut(ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        self.calls.lock().unwrap().push(context.clone());
        if input["value"] == "gated"
            && let Some(gate) = &self.gate
        {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        self.returned.store(true, Ordering::SeqCst);
        Ok(
            json!({"value": input["value"], "session": context.session_id(), "turn": context.turn_id(), "call": context.call_id()}),
        )
    }
}

async fn session(backend: Arc<Backend>, tools: ToolManager) -> RuntimeSession {
    RuntimeSessionBuilder::for_host(
        std::env::temp_dir().join("rara-host-fixture"),
        backend,
        tools,
    )
    .with_session_id("host-session")
    .with_system_prompt("Host-owned system context")
    .build()
    .await
    .expect("host session")
}

fn results(transcript: &[Message]) -> Vec<Value> {
    transcript
        .iter()
        .filter_map(|message| message.content.as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .cloned()
        .collect()
}

#[tokio::test]
async fn public_session_streams_tools_and_preserves_transcript_and_identity() {
    let backend = Arc::new(Backend::new(vec![
        calls_response(&["first", "second"]),
        text_response(),
    ]));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolManager::new();
    tools.register(Box::new(EchoTool {
        calls: calls.clone(),
        gate: None,
        returned: Arc::default(),
    }));
    let initial = vec![Message {
        role: "assistant".into(),
        content: json!("prior host history"),
    }];
    let session = RuntimeSessionBuilder::for_host(
        std::env::temp_dir().join("rara-host-fixture"),
        backend.clone(),
        tools,
    )
    .with_session_id("host-session")
    .with_system_prompt("Host-owned system context")
    .with_transcript(initial.clone())
    .build()
    .await
    .unwrap();
    let mut subscription = session.subscribe_from_snapshot().unwrap();
    let turn = session.submit("echo twice").await.unwrap();
    let id = turn.id().clone();
    let accounting = turn.accounting();
    let outcome = timeout(TIMEOUT, turn.wait()).await.unwrap().unwrap();
    assert!(accounting.snapshot().is_terminal());
    assert_eq!(accounting.snapshot().calls.len(), 2);
    assert_eq!(outcome.query_report.model_turns.len(), 2);
    assert_eq!(outcome.transcript[0], initial[0]);
    assert_eq!(session.transcript().await.unwrap(), outcome.transcript);
    let results = results(&outcome.transcript);
    assert_eq!(results.len(), 2);
    for (result, call_id) in results.iter().zip(["first", "second"]) {
        assert_eq!(result["tool_use_id"], call_id);
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(
            content,
            json!({"value": call_id, "session": "host-session", "turn": id.as_str(), "call": call_id})
        );
    }
    for (context, call_id) in calls.lock().unwrap().iter().zip(["first", "second"]) {
        assert_eq!(context.call_id(), Some(call_id));
        assert_eq!(context.session_id(), Some("host-session"));
        assert_eq!(context.turn_id(), Some(id.as_str()));
        assert_eq!(context.workspace_root(), Some(session.workspace_root()));
    }
    let requests = backend.requests.lock().unwrap().clone();
    assert_eq!(
        requests[0][0],
        Message {
            role: "system".into(),
            content: json!("Host-owned system context")
        }
    );
    assert_eq!(requests[1][0], requests[0][0]);
    let mut deltas = Vec::new();
    let mut result_ids = Vec::new();
    let mut cursor = 0;
    loop {
        let event = timeout(TIMEOUT, subscription.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(event.sequence > cursor);
        cursor = event.sequence;
        assert_eq!(event.turn_id.as_ref(), Some(&id));
        match event.event {
            RuntimeEvent::Assistant(AssistantEvent::Delta(delta)) => deltas.push(delta),
            RuntimeEvent::Tool(ToolEvent::Result { call_id, .. }) => result_ids.push(call_id),
            RuntimeEvent::Session(SessionEvent::TurnFinished) => break,
            _ => {}
        }
    }
    assert_eq!(deltas, ["streamed ", "tail", "streamed ", "tail"]);
    assert_eq!(result_ids, ["first", "second"]);
    session.replace_transcript(initial.clone()).await.unwrap();
    assert_eq!(session.transcript().await.unwrap(), initial);
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_waits_for_provider_return_and_fences_new_turns() {
    let gate = Arc::new(Gate::default());
    let mut backend = Backend::new(vec![text_response()]);
    backend.gate = Some(gate.clone());
    let session = session(Arc::new(backend), ToolManager::new()).await;
    let turn = session.submit("wait").await.unwrap();
    let id = turn.id().clone();
    timeout(TIMEOUT, gate.entered.notified()).await.unwrap();
    assert_eq!(session.cancel_turn(&id).await.unwrap(), id);
    assert!(
        gate.cancellation
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_cancelled()
    );
    assert!(matches!(
        session.submit("too early").await,
        Err(RuntimeSessionError::Busy { .. })
    ));
    assert!(matches!(
        session.interrupt_turn(&id).await,
        Err(RuntimeSessionError::StopInProgress { .. })
    ));
    assert!(matches!(
        session.snapshot().phase,
        RuntimeSessionPhase::Cancelling { .. }
    ));
    assert!(
        !session
            .replay_events(0)
            .unwrap()
            .iter()
            .any(|event| matches!(
                event.event,
                RuntimeEvent::Session(SessionEvent::TurnCancelled)
            ))
    );
    let mut completion = Box::pin(turn.wait());
    assert!(
        timeout(Duration::from_millis(25), &mut completion)
            .await
            .is_err()
    );
    gate.release.notify_one();
    let error = timeout(TIMEOUT, completion).await.unwrap().unwrap_err();
    assert!(matches!(error, RuntimeSessionError::Cancelled { .. }));
    assert_eq!(error.turn_outcome().unwrap().turn_id, id);
    assert_eq!(
        session.transcript().await.unwrap(),
        error.turn_outcome().unwrap().transcript
    );
    let events = session.replay_events(0).unwrap();
    assert!(matches!(
        events.last().unwrap().event,
        RuntimeEvent::Session(SessionEvent::TurnCancelled)
    ));
    assert!(events.iter().any(|event| matches!(&event.event, RuntimeEvent::Assistant(AssistantEvent::Delta(delta)) if delta == "tail")));
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_tool_keeps_completed_partial_results_without_running_later_calls() {
    let backend = Arc::new(Backend::new(vec![
        calls_response(&["first", "gated", "later"]),
        text_response(),
    ]));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(Gate::default());
    let returned = Arc::new(AtomicBool::new(false));
    let mut tools = ToolManager::new();
    tools.register(Box::new(EchoTool {
        calls: calls.clone(),
        gate: Some(gate.clone()),
        returned: returned.clone(),
    }));
    let session = session(backend.clone(), tools).await;
    let turn = session.submit("call tools").await.unwrap();
    timeout(TIMEOUT, gate.entered.notified()).await.unwrap();
    session.cancel().await.unwrap();
    assert!(calls.lock().unwrap()[1].is_cancelled());
    returned.store(false, Ordering::SeqCst);
    let mut completion = Box::pin(turn.wait());
    assert!(
        timeout(Duration::from_millis(25), &mut completion)
            .await
            .is_err()
    );
    assert!(!returned.load(Ordering::SeqCst));
    gate.release.notify_one();
    let error = timeout(TIMEOUT, completion).await.unwrap().unwrap_err();
    assert!(returned.load(Ordering::SeqCst));
    assert_eq!(calls.lock().unwrap().len(), 2);
    let partial_results = results(&error.turn_outcome().unwrap().transcript);
    assert_eq!(
        partial_results
            .iter()
            .map(|block| block["tool_use_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["first", "gated"]
    );
    let resumed = session.submit("continue after cancellation").await.unwrap();
    timeout(TIMEOUT, resumed.wait()).await.unwrap().unwrap();
    let requests = backend.requests.lock().unwrap().clone();
    let resumed_results = results(&requests[1]);
    assert_eq!(
        resumed_results.len(),
        3,
        "repair the unexecuted call before another request"
    );
    assert_eq!(&resumed_results[..2], partial_results.as_slice());
    assert_eq!(resumed_results[2]["tool_use_id"], "later");
    assert_eq!(resumed_results[2]["is_error"], true);
    assert_eq!(
        calls.lock().unwrap().len(),
        2,
        "recovery must not execute abandoned calls"
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_survives_a_cancelled_waiter_and_closes_after_execution() {
    let gate = Arc::new(Gate::default());
    let mut backend = Backend::new(vec![text_response()]);
    backend.gate = Some(gate.clone());
    let session = session(Arc::new(backend), ToolManager::new()).await;
    let turn = session.submit("wait").await.unwrap();
    timeout(TIMEOUT, gate.entered.notified()).await.unwrap();
    let mut snapshots = session.subscribe_snapshots();
    let closing = session.clone();
    let waiter = tokio::spawn(async move { closing.shutdown().await });
    timeout(TIMEOUT, async {
        while !matches!(snapshots.borrow().phase, RuntimeSessionPhase::Closing) {
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(matches!(
        session.submit("closed").await,
        Err(RuntimeSessionError::Closed)
    ));
    gate.release.notify_one();
    assert!(matches!(
        timeout(TIMEOUT, turn.wait()).await.unwrap(),
        Err(RuntimeSessionError::Cancelled { .. })
    ));
    timeout(TIMEOUT, session.shutdown()).await.unwrap().unwrap();
    session.shutdown().await.unwrap();
    assert_eq!(session.snapshot().phase, RuntimeSessionPhase::Closed);
}

#[tokio::test]
async fn replay_detects_expired_cursors_and_drains_before_closed() {
    let session = RuntimeSessionBuilder::for_host(
        std::env::temp_dir(),
        Arc::new(Backend::new(vec![text_response()])),
        ToolManager::new(),
    )
    .with_event_capacity(2)
    .build()
    .await
    .unwrap();
    let turn = session.submit("short turn").await.unwrap();
    timeout(TIMEOUT, turn.wait()).await.unwrap().unwrap();
    assert!(matches!(
        session.subscribe_after(0),
        Err(RuntimeSessionError::ResyncRequired { .. })
    ));
    let last = session.snapshot().last_sequence;
    let mut stream = session.subscribe_after(last - 2).unwrap();
    session.shutdown().await.unwrap();
    assert_eq!(stream.recv().await.unwrap().sequence, last - 1);
    assert_eq!(stream.recv().await.unwrap().sequence, last);
    assert!(matches!(
        stream.recv().await,
        Err(RuntimeSessionError::Closed)
    ));
}

#[tokio::test]
async fn provider_failure_retains_partial_history_and_terminal_evidence() {
    let session = session(Arc::new(Backend::new(Vec::new())), ToolManager::new()).await;
    let turn = session.submit("failure").await.unwrap();
    let accounting = turn.accounting();
    let error = timeout(TIMEOUT, turn.wait()).await.unwrap().unwrap_err();
    assert!(
        matches!(&error, RuntimeSessionError::Execution { message, .. } if message.contains("provider failed"))
    );
    assert_eq!(error.turn_outcome().unwrap().transcript.len(), 1);
    assert!(accounting.snapshot().is_terminal());
    assert_eq!(accounting.snapshot().calls.len(), 1);
    assert!(matches!(
        session.replay_events(0).unwrap().last().unwrap().event,
        RuntimeEvent::Session(SessionEvent::TurnFailed { .. })
    ));
    session.shutdown().await.unwrap();
}
