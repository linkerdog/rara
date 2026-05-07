use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
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
use crate::tool::ToolManager;
use crate::tool_result::ToolResultStore;
use crate::tools::planning::{EnterPlanModeTool, ExitPlanModeTool};
use crate::tools::todo::TodoWriteTool;
use crate::vectordb::VectorDB;
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

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; 8])
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

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; 8])
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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

#[tokio::test]
async fn plan_mode_consecutive_reasoning_only_turns_stop_after_three_continuations() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ProviderMetadata {
                provider: "deepseek".to_string(),
                key: "reasoning_content".to_string(),
                value: json!("First reasoning only."),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::ProviderMetadata {
                provider: "deepseek".to_string(),
                key: "reasoning_content".to_string(),
                value: json!("Second reasoning only."),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "plan-mode reachable text".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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
        .expect("consecutive reasoning-only plan turns should stop after one retry");

    let observed_messages = backend.observed_messages();
    let continuation_text = observed_messages
        .get(1)
        .and_then(|msgs| {
            msgs.iter().find_map(|message| {
                message
                    .content
                    .get(0)?
                    .get("text")?
                    .as_str()
                    .map(str::to_string)
            })
        })
        .expect("continuation message should be present");
    assert!(continuation_text.contains("\"phase\":\"ReasoningOnlyContinuationRequired\""));
    assert!(agent.history.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("plan-mode reachable text")
    }));
}

#[tokio::test]
async fn execute_mode_consecutive_reasoning_only_turns_stop_after_three_continuations() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ProviderMetadata {
                provider: "deepseek".to_string(),
                key: "reasoning_content".to_string(),
                value: json!("First reasoning only."),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::ProviderMetadata {
                provider: "deepseek".to_string(),
                key: "reasoning_content".to_string(),
                value: json!("Second reasoning only."),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "execute-mode reachable text".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Execute);

    agent
        .query_with_mode(
            "do a thing".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("consecutive reasoning-only execute turns should stop after one retry");

    let observed_messages = backend.observed_messages();
    let continuation_text = observed_messages
        .get(1)
        .and_then(|msgs| {
            msgs.iter().find_map(|message| {
                message
                    .content
                    .get(0)?
                    .get("text")?
                    .as_str()
                    .map(str::to_string)
            })
        })
        .expect("continuation message should be present");
    assert!(continuation_text.contains("\"phase\":\"ReasoningOnlyContinuationRequired\""));
    assert!(agent.history.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("execute-mode reachable text")
    }));
    let trace = agent.shared_runtime_context().observability.agent_turn;
    assert_eq!(trace.loop_outcome.as_deref(), Some("stopped"));
    assert_eq!(
        trace.continuation_phase.as_deref(),
        Some("reasoning_only_limit")
    );
    assert!(trace.reasoning_only);
    assert_eq!(trace.consecutive_reasoning_only_turns, 2);
}

#[tokio::test]
async fn suggestion_mode_auto_allows_read_only_bash_commands() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-readonly-bash".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git status --short" }),
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
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "inspect git state".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should auto-allow read-only bash");

    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
}

#[tokio::test]
async fn suggestion_mode_keeps_write_bash_commands_pending_approval() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "tool-write-bash".to_string(),
            name: "bash".to_string(),
            input: json!({ "command": "git push origin main" }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "push changes".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on write bash approval");

    assert!(agent.pending_approval.is_some());
    assert_eq!(backend.observed_messages().len(), 1);
}

#[tokio::test]
async fn denied_bash_approval_is_recorded_as_tool_failure_for_next_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-denied-bash".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git push origin main" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "I will retry when the user confirms the denial was accidental.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "push changes".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on write bash approval");
    assert!(agent.pending_approval.is_some());

    agent
        .answer_pending_approval_with_events(
            BashApprovalDecision::Suggestion,
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("denied approval should continue the agent turn");

    assert!(agent.pending_approval.is_none());
    let observed_messages = backend.observed_messages();
    assert_eq!(observed_messages.len(), 2);
    let resumed_history = &observed_messages[1];
    let tool_call_index = resumed_history
        .iter()
        .position(|message| {
            message.role == "assistant"
                && message.content.to_string().contains("tool-denied-bash")
                && message.content.to_string().contains("git push origin main")
        })
        .expect("assistant tool call should remain in history");
    let denial_result_index = resumed_history
        .iter()
        .position(|message| {
            let content = message.content.to_string();
            message.role == "user"
                && content.contains("\"tool_use_id\":\"tool-denied-bash\"")
                && content.contains("\"is_error\":true")
                && content.contains("rejected by user")
                && content.contains("The command was not run")
        })
        .expect("denied approval should be recorded as an errored tool result");
    let continuation_index = resumed_history
        .iter()
        .position(|message| {
            message.role == "user"
                && message.content.to_string().contains("<agent_runtime>")
                && message
                    .content
                    .to_string()
                    .contains("tool_results_available")
        })
        .expect("runtime continuation should follow the denied tool result");
    assert!(
        tool_call_index < denial_result_index,
        "tool result must follow its assistant tool call"
    );
    assert!(
        denial_result_index < continuation_index,
        "runtime continuation must follow the denied tool result"
    );
}

#[tokio::test]
async fn suggestion_mode_uses_escalated_sandbox_justification_for_approval() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "tool-escalated-bash".to_string(),
            name: "bash".to_string(),
            input: json!({
                "program": "cargo",
                "args": ["check"],
                "sandbox_permissions": "require_escalated",
                "justification": "Do you want to run cargo check outside the sandbox?",
                "prefix_rule": ["cargo", "check"]
            }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "run check outside sandbox".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on escalated bash approval");

    assert!(agent.pending_approval.is_some());
    assert!(
        agent.pending_user_input.is_none(),
        "bash approval should stay on the structured approval path"
    );
    assert_eq!(
        agent
            .pending_approval
            .as_ref()
            .and_then(|approval| approval.request.approval_prefix()),
        Some("cargo check".to_string())
    );
}

#[tokio::test]
async fn always_mode_still_requires_approval_for_escalated_sandbox_request() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "tool-escalated-bash".to_string(),
            name: "bash".to_string(),
            input: json!({
                "program": "cargo",
                "args": ["check"],
                "sandbox_permissions": "require_escalated"
            }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Always;

    agent
        .query_with_mode(
            "run check outside sandbox".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on escalated bash approval");

    assert!(agent.pending_approval.is_some());
    assert_eq!(backend.observed_messages().len(), 1);
}

#[tokio::test]
async fn full_access_mode_auto_allows_escalated_sandbox_request() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-escalated-bash".to_string(),
                name: "bash".to_string(),
                input: json!({
                    "program": "cargo",
                    "args": ["check"],
                    "sandbox_permissions": "require_escalated"
                }),
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
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Always;
    agent.set_full_access_mode(true);

    agent
        .query_with_mode(
            "run check with full access".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("full access should auto-allow escalated bash");

    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
}

#[tokio::test]
async fn approved_prefix_auto_allows_matching_escalated_request() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-escalated-bash".to_string(),
                name: "bash".to_string(),
                input: json!({
                    "program": "cargo",
                    "args": ["check"],
                    "sandbox_permissions": "require_escalated",
                    "prefix_rule": ["cargo", "check"]
                }),
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
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;
    agent.approved_bash_prefixes.push("cargo check".to_string());

    agent
        .query_with_mode(
            "run check outside sandbox".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should auto-allow escalated bash by approved prefix");

    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
}

#[tokio::test]
async fn plan_mode_allows_read_only_bash_commands() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-readonly-bash-plan".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git status --short" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Read-only inspection complete.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "inspect git state".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should allow read-only bash in plan mode");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
}

#[tokio::test]
async fn plan_mode_rejects_mutating_bash_commands_without_approval() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-write-bash-plan".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git push origin main" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "I will return a plan instead.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "push changes".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should reject mutating bash and continue");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
    assert!(agent.history.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("bash is read-only in plan mode")
    }));
}

#[tokio::test]
async fn approved_bash_prefix_auto_allows_later_matching_commands() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-first-push".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git push origin main" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "first push done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-second-push".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git push origin feature" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "second push done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "push once".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("first query should pause on approval");
    assert!(agent.pending_approval.is_some());

    agent
        .answer_pending_approval_with_events(
            BashApprovalDecision::Prefix,
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("prefix approval should execute pending command");
    assert_eq!(agent.approved_bash_prefixes, vec!["git push".to_string()]);

    agent
        .query_with_mode(
            "push matching prefix".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("matching prefix should auto-allow bash");

    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 4);
}

#[tokio::test]
async fn approved_bash_prefix_does_not_auto_allow_unapproved_shell_segments() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "tool-chained-push-rm".to_string(),
            name: "bash".to_string(),
            input: json!({ "command": "git push origin main && rm -rf target" }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;
    agent.approved_bash_prefixes.push("git push".to_string());

    agent
        .query_with_mode(
            "push and clean".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on the unapproved chained segment");

    assert!(agent.pending_approval.is_some());
    assert_eq!(backend.observed_messages().len(), 1);
}

#[tokio::test]
async fn checkpoints_user_message_before_first_model_turn() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let session_id = "checkpoint-before-model".to_string();
    let backend = Arc::new(CheckpointObserverBackend {
        session_manager: session_manager.clone(),
        session_id: session_id.clone(),
    });
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.session_id = session_id;
    agent.tool_result_store =
        ToolResultStore::new(rara_dir.join("tool-results")).expect("tool result store");

    agent
        .query_with_mode(
            "checkpoint me".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should succeed");
}

#[tokio::test]
async fn resumes_after_plan_approval_via_structured_continuation() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "Implemented the first plan step.".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    }]));

    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(VectorDB::new("data/lancedb")),
        session_manager,
        workspace,
    );
    agent.tool_result_store =
        ToolResultStore::new(rara_dir.join("tool-results")).expect("tool result store");
    agent.set_execution_mode(AgentExecutionMode::Plan);
    agent.current_plan = vec![PlanStep {
        step: "Modify workspace instruction discovery".to_string(),
        status: PlanStepStatus::Pending,
    }];

    agent
        .resume_after_plan_approval_with_events(
            false,
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("resume should succeed");

    let observed = backend.observed_messages();
    assert_eq!(observed.len(), 1);
    let runtime_texts = observed[0]
        .iter()
        .filter_map(|message| message.content.as_array())
        .flat_map(|blocks| blocks.iter())
        .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert!(
        runtime_texts
            .iter()
            .any(|text| text.contains("\"phase\": \"plan_approved\""))
    );
    assert!(
        runtime_texts
            .iter()
            .any(|text| text.contains("\"mode\": \"execute\""))
    );
    assert!(!runtime_texts.iter().any(|text| {
        text.contains("Implement the approved plan using the current repository state")
    }));
}

#[tokio::test]
async fn does_not_append_continuation_without_tools() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "final".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    }]));

    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode("hello".to_string(), super::super::AgentOutputMode::Silent)
        .await
        .expect("query should succeed");

    assert_eq!(backend.observed_messages().len(), 1);
    assert!(!agent.history.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("\"phase\": \"tool_results_available\"")
    }));
}

#[tokio::test]
async fn enter_plan_mode_tool_switches_to_read_only_planning() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "enter-plan".to_string(),
                name: "enter_plan_mode".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "The main issue is that planning and approval are coupled.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(EnterPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode(
            "review the planning implementation".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should enter planning mode and return analysis");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(!agent.last_query_produced_plan());
    assert!(agent.current_plan.is_empty());

    let observed_tools = backend.observed_tools();
    assert_eq!(observed_tools.len(), 2);
    assert!(observed_tools[0].contains(&"enter_plan_mode".to_string()));
    assert!(!observed_tools[1].contains(&"enter_plan_mode".to_string()));
}

#[tokio::test]
async fn enter_plan_mode_prevents_earlier_mutating_tool_in_same_batch() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![
                ContentBlock::ToolUse {
                    id: "write-before-plan".to_string(),
                    name: "bash".to_string(),
                    input: json!({ "command": "git push origin main" }),
                },
                ContentBlock::ToolUse {
                    id: "enter-plan".to_string(),
                    name: "enter_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "I will inspect first.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    tool_manager.register(Box::new(EnterPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode(
            "review then maybe implement".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should enter plan mode before executing batch tools");

    let history = agent
        .history
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(history.contains("bash is read-only in plan mode"));
    assert!(!history.contains("\"stdout\":\"ok"));
}

#[tokio::test]
async fn exit_plan_mode_persists_plan_and_waits_for_approval() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Update planning state\n- [pending] Add regression coverage\n</proposed_plan>".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Implemented the approved plan.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager.clone(),
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "plan the implementation".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should stop at exit plan approval");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(agent.has_pending_plan_exit_approval());
    let plan_file = session_manager.plan_file_path(&agent.session_id);
    let plan = std::fs::read_to_string(plan_file).expect("plan file should be persisted");
    assert!(plan.contains("- [pending] Update planning state"));
    assert!(plan.contains("- [pending] Add regression coverage"));

    agent
        .resume_after_plan_approval_with_events(
            false,
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("approved plan should resume execution");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Execute);
    assert!(!agent.has_pending_plan_exit_approval());
    let history = agent
        .history
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.contains("User has approved your plan. You can now start coding."));
    assert!(history.contains("Approved Plan"));
    assert!(history.contains("Implemented the approved plan."));
}

#[tokio::test]
async fn exit_plan_mode_accepts_structured_tool_plan_input() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "exit-plan".to_string(),
            name: "exit_plan_mode".to_string(),
            input: json!({
                "proposed_plan": {
                    "summary": "Repair malformed plan exits.",
                    "steps": [
                        { "step": "Capture proposed_plan from tool arguments", "status": "pending" },
                        { "step": "Persist the structured plan before approval", "status": "pending" }
                    ],
                    "validation": [
                        "cargo test exit_plan_mode -- --nocapture"
                    ]
                }
            }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend,
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager.clone(),
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "plan the implementation".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("structured tool plan should stop at plan approval");

    assert!(agent.has_pending_plan_exit_approval());
    assert_eq!(agent.current_plan.len(), 2);
    assert_eq!(
        agent.current_plan[0].step,
        "Capture proposed_plan from tool arguments"
    );
    assert_eq!(
        agent.plan_explanation.as_deref(),
        Some(
            "summary: Repair malformed plan exits.\nvalidation:\n- cargo test exit_plan_mode -- --nocapture"
        )
    );
    let plan_file = session_manager.plan_file_path(&agent.session_id);
    let plan = std::fs::read_to_string(plan_file).expect("plan file should be persisted");
    assert!(plan.contains("summary: Repair malformed plan exits."));
    assert!(plan.contains("- [pending] Capture proposed_plan from tool arguments"));
    assert!(plan.contains("cargo test exit_plan_mode -- --nocapture"));
}

#[tokio::test]
async fn exit_plan_mode_without_plan_gets_one_structured_repair_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "exit-plan-invalid".to_string(),
                name: "exit_plan_mode".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Fix plan exit handling\n</proposed_plan>"
                        .to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan-valid".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "exit without a concrete plan".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("invalid plan exit should get one repair turn");

    let observed_messages = backend.observed_messages();
    assert_eq!(observed_messages.len(), 2);
    assert!(observed_messages[1].iter().any(|message| {
        let content = message.content.to_string();
        content.contains("plan_exit_repair_required")
            && content.contains("Markdown headings")
            && content.contains("<proposed_plan>")
    }));
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(agent.has_pending_plan_exit_approval());
    assert!(agent.current_plan.iter().any(|step| {
        step.step == "Fix plan exit handling" && matches!(step.status, PlanStepStatus::Pending)
    }));
}

#[tokio::test]
async fn exit_plan_mode_requires_plan_from_same_assistant_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "exit-plan-first".to_string(),
                name: "exit_plan_mode".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "exit-plan-second".to_string(),
                name: "exit_plan_mode".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);
    agent.current_plan = vec![PlanStep {
        step: "stale plan step".to_string(),
        status: PlanStepStatus::Pending,
    }];

    let mut events = Vec::new();
    agent
        .query_with_mode_and_events(
            "exit with stale plan only".to_string(),
            super::super::AgentOutputMode::Silent,
            |event| events.push(event),
        )
        .await
        .expect("stale plan exit should get one repair attempt before stopping");

    assert_eq!(backend.observed_messages().len(), 2);
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(!agent.has_pending_plan_exit_approval());
    let repair_status_seen = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::Status(status)
                if status.contains("missing a structured proposed plan")
        )
    });
    assert!(repair_status_seen);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult {
            name,
            content,
            is_error: true,
        } if name == "exit_plan_mode"
            && content.contains("exit_plan_mode requires a proposed plan")
    )));
}

#[tokio::test]
async fn exit_plan_mode_with_unclosed_proposed_plan_reports_specific_error() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Update planning state".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan-first".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Update planning state".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan-second".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    let mut events = Vec::new();
    agent
        .query_with_mode_and_events(
            "exit with an incomplete plan".to_string(),
            super::super::AgentOutputMode::Silent,
            |event| events.push(event),
        )
        .await
        .expect("invalid plan exit should get one repair attempt before stopping");

    assert_eq!(backend.observed_messages().len(), 2);
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(!agent.has_pending_plan_exit_approval());
    assert!(agent.current_plan.is_empty());
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult {
            name,
            content,
            is_error: true,
        } if name == "exit_plan_mode"
            && content.contains("complete <proposed_plan>...</proposed_plan> block")
            && content.contains("</proposed_plan>")
    )));
}

#[tokio::test]
async fn continues_tool_loop_without_fixed_turn_cap() {
    let tool_turns = 205;
    let mut responses = (0..tool_turns)
        .map(|idx| LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: format!("tool-{idx}"),
                name: "stub_tool".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        })
        .collect::<Vec<_>>();
    responses.push(LlmResponse {
        content: vec![ContentBlock::Text {
            text: "Final answer after reviewing the tool results.".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    });
    let backend = Arc::new(SequencedBackend::new(responses));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode("loop".to_string(), super::super::AgentOutputMode::Silent)
        .await
        .expect("query should continue until the model returns a final answer");

    let observed_tools = backend.observed_tools();
    assert_eq!(
        observed_tools.len(),
        tool_turns + 1,
        "the agent should continue past the former fixed turn cap before the final answer"
    );
    assert!(agent.history.last().is_some_and(|message| {
        message
            .content
            .to_string()
            .contains("Final answer after reviewing the tool results.")
    }));
    assert!(
        agent
            .history
            .iter()
            .any(|message| message.content.to_string().contains("tool-204"))
    );
    assert_no_unresolved_tool_uses(&agent.history);
}

#[test]
fn strips_continue_inspection_control_tag() {
    let (cleaned, requested) =
        strip_continue_inspection_control("Need one more inspection pass.\n<continue_inspection/>");
    assert!(requested);
    assert_eq!(cleaned, "Need one more inspection pass.\n");

    let (cleaned, requested) = strip_continue_inspection_control("Final answer");
    assert!(!requested);
    assert_eq!(cleaned, "Final answer");
}

#[test]
fn parses_structured_plan_block() {
    let text = "<proposed_plan>\n- [in_progress] Inspect core agent loop\n- Review TUI rendering path\n1. Confirm current constraints\n</proposed_plan>\nFocus on agent.rs and tui/runtime.rs first.";
    let parsed = parse_plan_block(text).expect("plan block should parse");
    assert_eq!(
        parsed.0,
        vec![
            PlanStep {
                step: "Inspect core agent loop".to_string(),
                status: PlanStepStatus::InProgress,
            },
            PlanStep {
                step: "Review TUI rendering path".to_string(),
                status: PlanStepStatus::Pending,
            },
            PlanStep {
                step: "Confirm current constraints".to_string(),
                status: PlanStepStatus::Pending,
            },
        ]
    );
    assert_eq!(
        parsed.1.as_deref(),
        Some("Focus on agent.rs and tui/runtime.rs first.")
    );
}

#[test]
fn parses_structured_proposed_plan_fields_without_mixing_validation_into_steps() {
    let text = "<proposed_plan>\nsummary: Tighten plan exit handling.\nsteps:\n- [pending] Add structured plan prompt guidance\n- [pending] Parse only step entries from the steps section\nvalidation:\n- cargo test exit_plan_mode -- --nocapture\n- cargo check\n</proposed_plan>";
    let parsed = parse_plan_block(text).expect("plan block should parse");
    assert_eq!(
        parsed.0,
        vec![
            PlanStep {
                step: "Add structured plan prompt guidance".to_string(),
                status: PlanStepStatus::Pending,
            },
            PlanStep {
                step: "Parse only step entries from the steps section".to_string(),
                status: PlanStepStatus::Pending,
            },
        ]
    );
    let explanation = parsed.1.expect("structured metadata should be preserved");
    assert!(explanation.contains("summary: Tighten plan exit handling."));
    assert!(explanation.contains("validation:"));
    assert!(explanation.contains("cargo test exit_plan_mode -- --nocapture"));
    assert!(!parsed.0.iter().any(|step| step.step.contains("cargo test")));
}

#[test]
fn parses_structured_proposed_plan_headers_case_insensitively() {
    let text = "<proposed_plan>\nSummary: Tighten plan exit handling.\nSteps:\n- [pending] Add structured plan prompt guidance\nValidation:\n- cargo test exit_plan_mode -- --nocapture\n</proposed_plan>";
    let parsed = parse_plan_block(text).expect("plan block should parse");

    assert_eq!(
        parsed.0,
        vec![PlanStep {
            step: "Add structured plan prompt guidance".to_string(),
            status: PlanStepStatus::Pending,
        }]
    );
    let explanation = parsed.1.expect("structured metadata should be preserved");
    assert!(explanation.contains("summary: Tighten plan exit handling."));
    assert!(explanation.contains("validation:"));
    assert!(explanation.contains("cargo test exit_plan_mode -- --nocapture"));
}

#[test]
fn rejects_structured_proposed_plan_without_executable_steps() {
    let text = "<proposed_plan>\nsummary: Missing executable steps.\nsteps:\nvalidation:\n- cargo check\n</proposed_plan>";

    assert!(parse_plan_block(text).is_none());
}

#[test]
fn parses_structured_plan_from_exit_tool_input() {
    let parsed = parse_exit_plan_tool_input(&json!({
        "proposed_plan": {
            "summary": "Use tool arguments as the primary plan transport.",
            "steps": [
                { "step": "Add a proposed_plan schema to exit_plan_mode", "status": "completed" },
                { "step": "Capture structured tool input in plan mode", "status": "pending" }
            ],
            "validation": [
                "cargo test exit_plan_mode -- --nocapture"
            ]
        }
    }))
    .expect("structured tool input should parse");

    assert_eq!(
        parsed.0,
        vec![
            PlanStep {
                step: "Add a proposed_plan schema to exit_plan_mode".to_string(),
                status: PlanStepStatus::Completed,
            },
            PlanStep {
                step: "Capture structured tool input in plan mode".to_string(),
                status: PlanStepStatus::Pending,
            },
        ]
    );
    assert_eq!(
        parsed.1.as_deref(),
        Some(
            "summary: Use tool arguments as the primary plan transport.\nvalidation:\n- cargo test exit_plan_mode -- --nocapture"
        )
    );
}

#[test]
fn detects_unclosed_proposed_plan_block() {
    assert!(has_unclosed_proposed_plan_block(
        "<proposed_plan>\n- [pending] Update planning state"
    ));
    assert!(!has_unclosed_proposed_plan_block(
        "<proposed_plan>\n- [pending] Update planning state\n</proposed_plan>"
    ));
    assert!(has_unclosed_proposed_plan_block(
        "<proposed_plan>\n- [pending] First\n</proposed_plan>\n<proposed_plan>\n- [pending] Second"
    ));
    assert!(!has_unclosed_proposed_plan_block(
        "Ordinary planning answer without a structured plan."
    ));
}

#[test]
fn parses_request_user_input_block() {
    let text = "<request_user_input>\nquestion: Which path should we take first?\noption: Minimal | Keep the diff small and local.\noption: Broad | Reshape the module boundaries now.\n</request_user_input>\nNeed direction before editing.";
    let parsed = parse_request_user_input_block(text).expect("question block should parse");
    assert_eq!(
        parsed,
        PendingUserInput {
            question: "Which path should we take first?".to_string(),
            options: vec![
                (
                    "Minimal".to_string(),
                    "Keep the diff small and local.".to_string(),
                ),
                (
                    "Broad".to_string(),
                    "Reshape the module boundaries now.".to_string(),
                ),
            ],
            note: Some("Need direction before editing.".to_string()),
        }
    );
}

fn new_planning_agent() -> Agent {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(SequencedBackend::new(Vec::new())),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);
    agent
}

#[test]
fn shallow_initial_plan_continues_even_after_plan_update() {
    let mut agent = new_planning_agent();
    agent.current_plan = vec![PlanStep {
        step: "Inspect code".to_string(),
        status: PlanStepStatus::Pending,
    }];

    assert!(agent.should_continue_plan_without_tools(true, false, true, false, 0,));
}

#[test]
fn missing_minimum_review_evidence_continues_without_plan_update() {
    let mut agent = new_planning_agent();
    agent.inspection_progress.source_reads = 1;

    assert!(agent.should_continue_plan_without_tools(false, false, true, false, 1,));
}

#[test]
fn reasoning_only_plan_turn_signals_continuation() {
    // should_continue_plan_without_tools always returns true for
    // reasoning-only turns. The consecutive-turn cap is enforced in
    // run_agent_loop_with_limit.
    let agent = new_planning_agent();

    assert!(agent.should_continue_plan_without_tools(false, false, false, true, 0));
    assert!(agent.should_continue_plan_without_tools(false, false, false, true, 1));
}

#[test]
fn execute_mode_continuation_requires_structured_inspection_marker() {
    let mut agent = new_planning_agent();
    agent.set_execution_mode(AgentExecutionMode::Execute);
    agent.inspection_progress.source_reads = 1;

    // Without continue_inspection or reasoning, bare text doesn't continue.
    assert!(!agent.should_continue_execute_without_tools(false, true, false));
    // With continue_inspection, text continues.
    assert!(agent.should_continue_execute_without_tools(true, true, false));
    // Reasoning-only turn always continues at this level (cap in agent loop).
    assert!(agent.should_continue_execute_without_tools(false, false, true));
    // Same — reasoning-only continuation is turn-agnostic here.
    assert!(agent.should_continue_execute_without_tools(false, false, true));

    agent.inspection_progress.source_reads = 2;
    assert!(agent.should_continue_execute_without_tools(true, true, false));
}

#[test]
fn advances_plan_steps_during_execute_mode() {
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(SequencedBackend::new(Vec::new())),
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.set_execution_mode(AgentExecutionMode::Execute);
    agent.current_plan = vec![
        PlanStep {
            step: "Inspect code".to_string(),
            status: PlanStepStatus::Pending,
        },
        PlanStep {
            step: "Apply changes".to_string(),
            status: PlanStepStatus::Pending,
        },
    ];

    agent.ensure_active_plan_step();
    assert_eq!(agent.current_plan[0].status, PlanStepStatus::InProgress);
    assert_eq!(agent.current_plan[1].status, PlanStepStatus::Pending);

    agent.advance_plan_step();
    assert_eq!(agent.current_plan[0].status, PlanStepStatus::Completed);
    assert_eq!(agent.current_plan[1].status, PlanStepStatus::InProgress);
}

#[test]
fn completes_only_active_plan_step_on_finish() {
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(SequencedBackend::new(Vec::new())),
        Arc::new(VectorDB::new("data/lancedb")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.set_execution_mode(AgentExecutionMode::Execute);
    agent.current_plan = vec![
        PlanStep {
            step: "Inspect code".to_string(),
            status: PlanStepStatus::Completed,
        },
        PlanStep {
            step: "Apply changes".to_string(),
            status: PlanStepStatus::InProgress,
        },
        PlanStep {
            step: "Summarize".to_string(),
            status: PlanStepStatus::Pending,
        },
    ];

    agent.complete_active_plan_step();

    assert_eq!(agent.current_plan[0].status, PlanStepStatus::Completed);
    assert_eq!(agent.current_plan[1].status, PlanStepStatus::Completed);
    assert_eq!(agent.current_plan[2].status, PlanStepStatus::Pending);
}

fn assert_no_unresolved_tool_uses(history: &[crate::agent::Message]) {
    let mut pending = Vec::new();
    for message in history {
        if let Some(items) = message.content.as_array() {
            for item in items {
                match item.get("type").and_then(serde_json::Value::as_str) {
                    Some("tool_use") if message.role == "assistant" => {
                        if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                            pending.push(id.to_string());
                        }
                    }
                    Some("tool_result") if message.role == "user" => {
                        if let Some(id) =
                            item.get("tool_use_id").and_then(serde_json::Value::as_str)
                        {
                            if let Some(pos) =
                                pending.iter().position(|pending_id| pending_id == id)
                            {
                                pending.remove(pos);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    assert!(pending.is_empty(), "unresolved tool uses: {pending:?}");
}
