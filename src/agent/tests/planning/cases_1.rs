use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use rara_memory::memory_handle::MemoryHandle;
use rara_tools::planning::{EnterPlanModeTool, ExitPlanModeTool};
use rara_tools::tool::ToolManager;
use serde_json::json;
use tempfile::tempdir;

use super::support::{SequencedBackend, StubBashTool, StubTool, test_runtime_storage};
use crate::agent::planning::{
    has_unclosed_proposed_plan_block, parse_exit_plan_tool_input, parse_plan_block,
    parse_request_user_input_block, strip_continue_inspection_control,
};
use crate::agent::{
    Agent, AgentEvent, AgentExecutionMode, BashApprovalDecision, ContentBlock, PendingUserInput,
    PlanStep, PlanStepStatus, RuntimeContinuationPhase,
};
use crate::llm::{LlmBackend, LlmResponse, TokenUsage};
use crate::session::SessionManager;
use crate::tool_result::ToolResultStore;
use crate::tools::todo::TodoWriteTool;
use crate::workspace::WorkspaceMemory;

struct CheckpointObserverBackend {
    session_manager: Arc<SessionManager>,
    session_id: String,
}

#[async_trait]
impl LlmBackend for CheckpointObserverBackend {
    async fn ask(
        &self,
        _messages: &[crate::agent::Message],
        _tools: &[serde_json::Value],
    ) -> Result<LlmResponse> {
        let persisted = self
            .session_manager
            .load_thread_history(&self.session_id)
            .expect("user message should be checkpointed before model call");
        assert!(persisted.iter().any(|message| {
            message.role == "user" && message.content.to_string().contains("checkpoint me")
        }));
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "checkpoint observed".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }
    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> Result<String> {
        Ok("summary".to_string())
    }
}

struct RecoverableRuntimeErrorBackend {
    calls: Mutex<usize>,
    observed_messages: Mutex<Vec<Vec<crate::agent::Message>>>,
}

impl RecoverableRuntimeErrorBackend {
    fn new() -> Self {
        Self {
            calls: Mutex::new(0),
            observed_messages: Mutex::new(Vec::new()),
        }
    }

    fn observed_messages(&self) -> Vec<Vec<crate::agent::Message>> {
        self.observed_messages.lock().expect("lock").clone()
    }
}

#[async_trait]
impl LlmBackend for RecoverableRuntimeErrorBackend {
    async fn ask(
        &self,
        messages: &[crate::agent::Message],
        _tools: &[serde_json::Value],
    ) -> Result<LlmResponse> {
        self.observed_messages
            .lock()
            .expect("lock")
            .push(messages.to_vec());
        let mut calls = self.calls.lock().expect("lock");
        *calls += 1;
        if *calls == 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "No space left on device (os error 28)",
            )
            .into());
        }
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Recovered after inspecting the runtime error.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        })
    }
    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> Result<String> {
        Ok("summary".to_string())
    }
}

#[tokio::test]
async fn emits_model_request_and_response_events() {
    let backend = Arc::new(
        SequencedBackend::new(vec![LlmResponse {
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage {
                input_tokens: 11,
                output_tokens: 22,
                ..TokenUsage::default()
            }),
        }])
        .with_model_label("test-model"),
    );

    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    let mut events = Vec::new();
    agent
        .query_with_mode_and_events(
            "hello".to_string(),
            super::super::AgentOutputMode::Silent,
            |event| events.push(event),
        )
        .await
        .expect("query should succeed");

    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ModelRequest {
                model,
                input_tokens: 0
            } if model == "test-model"
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ModelResponse {
                model,
                output_tokens: 22,
                finish_reason: Some(reason)
            } if model == "test-model" && reason == "end_turn"
        )
    }));
}

#[tokio::test]
async fn appends_continuation_after_tool_result() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "stub_tool".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode("do work".to_string(), super::super::AgentOutputMode::Silent)
        .await
        .expect("query should succeed");

    let observed = backend.observed_messages();
    assert_eq!(observed.len(), 2);
    let second_round = &observed[1];
    let continuation =
        agent.runtime_continuation_message(RuntimeContinuationPhase::ToolResultsAvailable, 1);
    assert!(
        second_round
            .iter()
            .any(|message| message.content == continuation.content)
    );
    assert!(
        second_round
            .iter()
            .any(|message| { message.content.to_string().contains("tool_result") })
    );
}

#[tokio::test]
async fn visible_text_before_tool_call_does_not_end_agent_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "I will inspect the file first.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "stub_tool".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "The tool result is handled.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = events.clone();

    agent
        .query_with_mode_and_events(
            "continue".to_string(),
            super::super::AgentOutputMode::Silent,
            move |event| captured_events.lock().expect("events").push(event),
        )
        .await
        .expect("query should continue through tool call");

    let events = events.lock().expect("events");
    let text_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::AssistantText(text) if text.contains("inspect the file")
            )
        })
        .expect("visible text event");
    let tool_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::ToolUse { name, .. } if name == "stub_tool"
            )
        })
        .expect("tool use event");
    assert!(
        text_idx < tool_idx,
        "visible assistant text should render before the tool call is executed"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::ToolResult {
                    name,
                    is_error: false,
                    ..
                } if name == "stub_tool"
            )
        }),
        "tool call should still execute after visible text"
    );
    drop(events);

    let observed = backend.observed_messages();
    assert_eq!(
        observed.len(),
        2,
        "tool result should trigger a follow-up model turn"
    );
    assert!(
        observed[1]
            .iter()
            .any(|message| message.content.to_string().contains("tool_result")),
        "follow-up model turn should receive the tool result"
    );
}

#[tokio::test]
async fn raw_leading_think_is_not_persisted_as_assistant_context_text() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "<think>private reasoning</think>\nVisible answer.<｜end▁of▁sentence｜>"
                    .to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Second answer.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let tool_manager = ToolManager::new();
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode_and_events(
            "first".to_string(),
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("first query");
    agent
        .query_with_mode_and_events(
            "second".to_string(),
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("second query");

    let history_text = serde_json::to_string(&agent.history).expect("history json");
    assert!(history_text.contains("Visible answer."));
    assert!(!history_text.contains("<think>"));
    assert!(!history_text.contains("private reasoning"));

    let observed = backend.observed_messages();
    assert_eq!(observed.len(), 2);
    let second_assistant_text = observed[1]
        .iter()
        .filter(|message| message.role == "assistant")
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(second_assistant_text.contains("Visible answer."));
    assert!(!second_assistant_text.contains("<think>"));
    assert!(!second_assistant_text.contains("private reasoning"));
}

#[tokio::test]
async fn todo_write_updates_session_state_and_emits_event() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "todo-tool-1".to_string(),
                name: "todo_write".to_string(),
                input: json!({
                    "todos": [
                        {"content": "Implement todo runtime", "status": "in_progress"},
                        {"content": "Run focused tests", "status": "pending"}
                    ]
                }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Todo state is recorded.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(TodoWriteTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager.clone(),
        workspace,
    );
    agent.session_id = "todo-session".to_string();
    let events = Arc::new(Mutex::new(Vec::new()));

    agent
        .query_with_mode_and_events(
            "track the implementation".to_string(),
            super::super::AgentOutputMode::Silent,
            {
                let events = events.clone();
                move |event| events.lock().expect("events").push(event)
            },
        )
        .await
        .expect("query should succeed");

    let state = agent.todo_state.expect("agent should keep todo state");
    assert_eq!(state.items.len(), 2);
    assert_eq!(
        state.summary().active_item.as_deref(),
        Some("Implement todo runtime")
    );
    assert_eq!(
        state.summary().active_label.as_deref(),
        Some("Implement todo runtime")
    );
    assert_eq!(
        session_manager
            .load_todo_state("todo-session")
            .expect("todo state should load"),
        Some(state.clone())
    );
    assert!(
        events
            .lock()
            .expect("events")
            .iter()
            .any(|event| matches!(event, AgentEvent::TodoUpdated(updated) if *updated == state))
    );
}

#[tokio::test]
async fn todo_write_persistence_failure_warns_without_aborting_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "todo-tool-1".to_string(),
                name: "todo_write".to_string(),
                input: json!({
                    "todos": [
                        {"content": "Keep going after persistence failure", "status": "in_progress"}
                    ]
                }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Todo state is still usable.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(TodoWriteTool));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let rara_dir = root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    let blocked_legacy_path = rara_dir.join("sessions");
    std::fs::write(&blocked_legacy_path, "not a directory").expect("blocked sessions path");
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: blocked_legacy_path,
    });
    let workspace = Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone()));
    let mut agent = Agent::new(
        tool_manager,
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    agent.session_id = "todo-session".to_string();
    let events = Arc::new(Mutex::new(Vec::new()));

    agent
        .query_with_mode_and_events(
            "track the implementation".to_string(),
            super::super::AgentOutputMode::Silent,
            {
                let events = events.clone();
                move |event| events.lock().expect("events").push(event)
            },
        )
        .await
        .expect("todo persistence failure should not abort the turn");

    let state = agent.todo_state.expect("agent should keep todo state");
    assert_eq!(state.items.len(), 1);
    let events = events.lock().expect("events");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Status(message)
            if message.contains("Warning: failed to persist todo state")
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::TodoUpdated(updated) if *updated == state))
    );
}

#[test]
fn plan_mode_does_not_expose_todo_write_schema() {
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(TodoWriteTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        Arc::new(SequencedBackend::new(Vec::new())),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent.set_execution_mode(AgentExecutionMode::Plan);
    let plan_tool_names = agent
        .visible_tool_schemas()
        .into_iter()
        .filter_map(|schema| {
            schema
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert!(!plan_tool_names.iter().any(|name| name == "todo_write"));

    agent.set_execution_mode(AgentExecutionMode::Execute);
    let execute_tool_names = agent
        .visible_tool_schemas()
        .into_iter()
        .filter_map(|schema| {
            schema
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert!(execute_tool_names.iter().any(|name| name == "todo_write"));
}

#[tokio::test]
async fn recoverable_runtime_error_is_returned_to_model_once() {
    let backend = Arc::new(RecoverableRuntimeErrorBackend::new());
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode(
            "continue after local failure".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("recoverable runtime error should be surfaced to the model");

    let observed = backend.observed_messages();
    assert_eq!(observed.len(), 2);
    let second_round = observed[1]
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(second_round.contains("<agent_runtime_error>"));
    assert!(second_round.contains("storage_full"));
    assert!(second_round.contains("No space left on device"));
    assert!(agent.history.last().is_some_and(|message| {
        message
            .content
            .to_string()
            .contains("Recovered after inspecting the runtime error.")
    }));
}

#[tokio::test]
async fn reasoning_only_turn_is_not_persisted_as_empty_assistant_message() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ProviderMetadata {
                provider: "deepseek".to_string(),
                key: "reasoning_content".to_string(),
                value: json!("internal planning only"),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Visible answer after structured continuation.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode(
            "list your todo".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("reasoning-only response should continue once");

    let observed_messages = backend.observed_messages();
    assert_eq!(observed_messages.len(), 2);
    assert!(observed_messages[1].iter().any(|message| {
        message
            .content
            .to_string()
            .contains("reasoning_only_continuation_required")
    }));
    let trace = agent.shared_runtime_context().observability.agent_turn;
    assert_eq!(trace.loop_outcome.as_deref(), Some("stopped"));
    assert_eq!(
        trace.continuation_phase.as_deref(),
        Some("final_no_tool_response")
    );
    assert!(trace.had_text_response);
    assert!(!trace.reasoning_only);
    let assistant_messages = agent
        .history
        .iter()
        .filter(|message| message.role == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(assistant_messages.len(), 1);
    assert!(
        assistant_messages[0]
            .content
            .to_string()
            .contains("Visible answer after structured continuation.")
    );
}

#[tokio::test]
async fn plan_mode_reasoning_only_initial_turn_continues_to_next_model_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ProviderMetadata {
                provider: "deepseek".to_string(),
                key: "reasoning_content".to_string(),
                value: json!("Need to inspect cells before planning."),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "I need to inspect the TUI cell code before proposing the split.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "plan the cells split".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("reasoning-only plan turn should continue once");

    let observed_messages = backend.observed_messages();
    assert_eq!(observed_messages.len(), 2);
    assert!(observed_messages[1].iter().any(|message| {
        message
            .content
            .to_string()
            .contains("plan_continuation_required")
    }));
    assert!(agent.history.last().is_some_and(|message| {
        message
            .content
            .to_string()
            .contains("I need to inspect the TUI cell code")
    }));
}
