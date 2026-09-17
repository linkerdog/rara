// Host fixtures never use ambient providers or execute real shell commands.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use rara_tools::planning::ExitPlanModeTool;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

use super::*;
use crate::agent::AgentExecutionMode;
use crate::runtime_client::RuntimeClient;
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};
use crate::runtime_control::{InputDiscardReason, InputEvent, RuntimeControlEvent};
use crate::{
    AgentOutputMode, ContentBlock, LlmBackend, LlmResponse, Message, RaraConfig, RuntimeEvent,
    SessionEvent, TokenUsage, Tool, ToolCallContext, ToolError, ToolManager, ToolProgressEvent,
};

mod approvals;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const QUESTION: &str = "<request_user_input>\nquestion: Which path?\noption: Minimal | Keep the diff small.\noption: Broad | Expand.\n</request_user_input>\nChoose one.";

struct ScriptedBackend {
    responses: Mutex<VecDeque<LlmResponse>>,
    requests: Mutex<Vec<Vec<Message>>>,
    gate: Option<Notify>,
    started: Notify,
}

impl ScriptedBackend {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            gate: None,
            started: Notify::new(),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("requests").len()
    }
}

#[async_trait]
impl LlmBackend for ScriptedBackend {
    async fn ask(&self, messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        self.requests
            .lock()
            .expect("requests")
            .push(messages.to_vec());
        self.started.notify_one();
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        self.responses
            .lock()
            .expect("responses")
            .pop_front()
            .ok_or_else(|| anyhow!("unexpected extra provider call"))
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Err(anyhow!("input fixture must not summarize"))
    }
}

#[derive(Default)]
struct RecordedShell {
    calls: Mutex<Vec<(Value, String, String)>>,
}

struct RecordingTool(Arc<RecordedShell>);

#[async_trait]
impl Tool for RecordingTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Record an approved command without running it"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn call(&self, _input: Value) -> std::result::Result<Value, ToolError> {
        panic!("approval must preserve the tool execution context")
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(ToolProgressEvent) + Send),
    ) -> std::result::Result<Value, ToolError> {
        self.0.calls.lock().expect("shell calls").push((
            input,
            context.turn_id().expect("turn identity").to_owned(),
            context
                .call_id()
                .expect("provider tool identity")
                .to_owned(),
        ));
        Ok(json!({"recorded": true}))
    }
}

async fn session(
    root: &std::path::Path,
    backend: Arc<ScriptedBackend>,
    shell: Arc<RecordedShell>,
    mode: AgentExecutionMode,
) -> RuntimeSession {
    let mut tools = ToolManager::new();
    tools.register(Box::new(RecordingTool(shell)));
    tools.register(Box::new(ExitPlanModeTool));
    let mut config = RaraConfig::default();
    config.builtin_plugins.nowledge_mem.enabled = false;
    let options = RuntimeBootstrapOptions::with_plugin_dirs(Vec::new())
        .with_rara_home(Some(root.join("state")))
        .with_backend(Some(backend))
        .with_tool_manager(Some(tools))
        .with_extension_discovery(false)
        .with_memory_facilities(false)
        .with_transcript_persistence(false);
    let bootstrap =
        initialize_rara_context_for_workspace_with_options(&config, Some(root), None, options)
            .await
            .expect("isolated bootstrap");
    let mut client = RuntimeClient::from_bootstrap(bootstrap).await;
    // Set the native mode before handing the entire agent to its session owner.
    client
        .agent_mut()
        .as_mut()
        .expect("agent")
        .set_execution_mode(mode);
    RuntimeSession::start(client, 32).expect("isolated session")
}

fn response(content: Vec<ContentBlock>, tokens: u32) -> LlmResponse {
    let uses_tool = content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }));
    LlmResponse {
        content,
        stop_reason: Some(if uses_tool { "tool_use" } else { "end_turn" }.into()),
        usage: Some(TokenUsage {
            input_tokens: tokens,
            output_tokens: 1,
            ..TokenUsage::default()
        }),
    }
}

fn text_response(text: &str, tokens: u32) -> LlmResponse {
    response(vec![ContentBlock::Text { text: text.into() }], tokens)
}

fn user_answer(waiting_turn: &RuntimeTurnId) -> RuntimeInput {
    RuntimeInput::Answer {
        waiting_turn: waiting_turn.clone(),
        answer: RuntimeInputAnswer::User {
            answer: "Minimal".into(),
        },
    }
}

async fn finish(turn: RuntimeTurn) -> RuntimeTurnOutcome {
    let ledger = turn.accounting();
    let outcome = timeout(TEST_TIMEOUT, turn.wait())
        .await
        .expect("completion")
        .expect("outcome");
    assert!(
        ledger.snapshot().is_terminal(),
        "approval continuation must release its inference lease"
    );
    assert_eq!(
        ledger.snapshot().calls.len(),
        outcome.query_report.model_turns.len(),
        "each answer owns its model observations"
    );
    outcome
}

async fn events(session: &RuntimeSession) -> Vec<RuntimeControlEvent> {
    let last_sequence = session.snapshot().last_sequence;
    let mut stream = session.subscribe_after(0).expect("replay window");
    let mut events = Vec::new();
    while stream.cursor() < last_sequence {
        events.push(
            timeout(TEST_TIMEOUT, stream.recv())
                .await
                .expect("event timeout")
                .expect("event"),
        );
    }
    events
}

#[tokio::test]
async fn successive_questions_fence_replies_and_replay_the_original_wait() {
    let root = tempfile::tempdir().expect("workspace");
    let mut scripted = ScriptedBackend::new(vec![
        text_response(QUESTION, 10),
        text_response(QUESTION, 20),
        text_response("done", 30),
    ]);
    scripted.gate = Some(Notify::new());
    let backend = Arc::new(scripted);
    let session = session(
        root.path(),
        backend.clone(),
        Arc::default(),
        AgentExecutionMode::Plan,
    )
    .await;
    let first = session
        .submit_input(RuntimeInput::Prompt("ask me".into()))
        .await
        .expect("first");
    let first_id = first.id().clone();
    timeout(TEST_TIMEOUT, backend.started.notified())
        .await
        .expect("provider started");
    assert!(
        matches!(session.submit_input(RuntimeInput::FollowUp("later".into())).await, Err(RuntimeSessionError::Busy { active_turn }) if active_turn == first_id)
    );
    backend.gate.as_ref().unwrap().notify_one();
    let outcome = finish(first).await;
    assert_eq!(
        outcome.query_report.model_turns[0]
            .usage
            .unwrap()
            .input_tokens,
        10
    );
    let snapshot = session.snapshot();
    assert_eq!(
        snapshot.phase,
        RuntimeSessionPhase::AwaitingInput {
            turn_id: first_id.clone()
        }
    );
    let pending = snapshot.pending_input.expect("pending question");
    assert!(
        matches!(&pending.kind, RuntimePendingInputKind::User { question, options, note } if question == "Which path?" && options.len() == 2 && note.as_deref() == Some("Choose one."))
    );
    for input in [
        RuntimeInput::Prompt("replace".into()),
        RuntimeInput::FollowUp("replace".into()),
    ] {
        assert!(matches!(
            session.submit_input(input).await,
            Err(RuntimeSessionError::AwaitingInput { .. })
        ));
    }
    assert!(matches!(
        session.replace_transcript(Vec::new()).await,
        Err(RuntimeSessionError::AwaitingInput { .. })
    ));
    assert!(matches!(
        session
            .submit_input(RuntimeInput::Answer {
                waiting_turn: first_id.clone(),
                answer: RuntimeInputAnswer::Shell {
                    decision: crate::ShellApprovalDecision::Once
                }
            })
            .await,
        Err(RuntimeSessionError::InputKindMismatch)
    ));
    assert_eq!(backend.request_count(), 1);
    let replay = events(&session).await;
    let requested = replay.iter().position(|event| matches!(&event.event, RuntimeEvent::Input(InputEvent::Requested { pending: item }) if **item == pending)).expect("wait event");
    let terminal = replay.iter().position(|event| matches!(&event.event, RuntimeEvent::Session(SessionEvent::TurnFinished { reason }) if reason.as_deref() == Some("awaiting_input"))).expect("wait terminal");
    assert!(requested < terminal);
    assert_eq!(
        replay,
        events(&session).await,
        "replay keeps event IDs and sequences"
    );

    let second = session
        .submit_input(user_answer(&first_id))
        .await
        .expect("answer");
    let second_id = second.id().clone();
    assert_ne!(first_id, second_id);
    timeout(TEST_TIMEOUT, backend.started.notified())
        .await
        .expect("answer provider");
    assert!(matches!(
        session.submit_input(user_answer(&first_id)).await,
        Err(RuntimeSessionError::Busy { .. })
    ));
    backend.gate.as_ref().unwrap().notify_one();
    assert_eq!(
        finish(second).await.query_report.model_turns[0]
            .usage
            .unwrap()
            .input_tokens,
        20
    );
    assert!(
        matches!(session.submit_input(user_answer(&first_id)).await, Err(RuntimeSessionError::StaleInput { expected, waiting }) if expected == first_id && waiting == second_id)
    );
    assert!(matches!(
        session.cancel_turn(&first_id).await,
        Err(RuntimeSessionError::StaleInput { .. })
    ));
    assert_eq!(backend.request_count(), 2);
    let third = session
        .submit_input(user_answer(&second_id))
        .await
        .expect("second answer");
    timeout(TEST_TIMEOUT, backend.started.notified())
        .await
        .expect("second answer provider");
    backend.gate.as_ref().unwrap().notify_one();
    finish(third).await;
    assert_eq!(session.snapshot().phase, RuntimeSessionPhase::Idle);
    assert!(session.snapshot().pending_input.is_none());
    assert!(matches!(
        session.submit_input(user_answer(&second_id)).await,
        Err(RuntimeSessionError::NoPendingInput)
    ));
    assert_eq!(backend.request_count(), 3);
    assert!(events(&session).await.iter().any(|event| matches!(&event.event, RuntimeEvent::Input(InputEvent::Answered { waiting_turn }) if waiting_turn == first_id.as_str()) && event.turn_id.as_deref() == Some(second_id.as_str())));
    session.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn stop_and_close_discard_waits_without_an_extra_terminal_turn() {
    for reason in [
        InputDiscardReason::Cancelled,
        InputDiscardReason::Interrupted,
        InputDiscardReason::Shutdown,
        InputDiscardReason::Superseded,
    ] {
        let root = tempfile::tempdir().expect("workspace");
        let backend = Arc::new(ScriptedBackend::new(vec![
            text_response(QUESTION, 10),
            text_response("done", 20),
        ]));
        let session = session(
            root.path(),
            backend.clone(),
            Arc::default(),
            AgentExecutionMode::Plan,
        )
        .await;
        let first = finish(
            session
                .submit_input(RuntimeInput::Prompt("ask".into()))
                .await
                .expect("turn"),
        )
        .await;
        match reason {
            InputDiscardReason::Cancelled => {
                session
                    .cancel_turn(&first.turn_id)
                    .await
                    .expect("cancel wait");
            }
            InputDiscardReason::Interrupted => {
                session
                    .interrupt_turn(&first.turn_id)
                    .await
                    .expect("interrupt wait");
            }
            InputDiscardReason::Shutdown => session.shutdown().await.expect("close wait"),
            InputDiscardReason::Superseded => {
                finish(
                    session
                        .submit("legacy replacement", AgentOutputMode::Silent)
                        .await
                        .expect("legacy"),
                )
                .await;
            }
        }
        assert!(session.snapshot().pending_input.is_none());
        assert!(matches!(
            session.submit_input(user_answer(&first.turn_id)).await,
            Err(RuntimeSessionError::NoPendingInput | RuntimeSessionError::Closed)
        ));
        assert_eq!(
            backend.request_count(),
            if reason == InputDiscardReason::Superseded {
                2
            } else {
                1
            }
        );
        let replay = events(&session).await;
        assert!(replay.iter().any(|event| matches!(&event.event, RuntimeEvent::Input(InputEvent::Discarded { waiting_turn, reason: actual }) if waiting_turn == first.turn_id.as_str() && *actual == reason)));
        assert_eq!(
            replay
                .iter()
                .filter(
                    |event| event.turn_id.as_deref() == Some(first.turn_id.as_str())
                        && matches!(
                            event.event,
                            RuntimeEvent::Session(
                                SessionEvent::TurnFinished { .. }
                                    | SessionEvent::TurnCancelled
                                    | SessionEvent::TurnInterrupted
                                    | SessionEvent::TurnFailed { .. }
                            )
                        )
                )
                .count(),
            1
        );
        session.shutdown().await.expect("shutdown");
    }
}
