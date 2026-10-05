use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use rara_memory::memory_handle::MemoryHandle;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError, ToolManager};
use serde_json::{Value, json};

use super::support::test_runtime_storage;
use crate::agent::{Agent, AgentEvent, AgentOutputMode, Message};
use crate::llm::{ContentBlock, LlmBackend, LlmResponse};
use crate::session::SessionManager;

struct CheckpointTool {
    sessions: Arc<SessionManager>,
    session_id: String,
}

#[tool_spec(name = "checkpoint_probe", description = "Observe persisted assistant intent", input_schema = { "type": "object" })]
#[async_trait]
impl Tool for CheckpointTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        let persisted = self
            .sessions
            .load_thread_history(&self.session_id)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        assert!(persisted.iter().any(|message| message.role == "assistant"
            && message.content.as_array().is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block["type"] == "tool_use" && block["id"] == "probe-1")
            })));
        Ok(json!({"persisted_intent": true}))
    }
}

struct CheckpointBackend {
    sessions: Arc<SessionManager>,
    session_id: String,
    calls: AtomicUsize,
}

#[async_trait]
impl LlmBackend for CheckpointBackend {
    async fn ask(&self, messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(LlmResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "probe-1".into(),
                    name: "checkpoint_probe".into(),
                    input: json!({}),
                }],
                stop_reason: Some("tool_use".into()),
                usage: None,
            });
        }
        let persisted = self.sessions.load_thread_history(&self.session_id)?;
        for history in [messages, persisted.as_slice()] {
            assert!(
                history
                    .iter()
                    .any(
                        |message| message.content.as_array().is_some_and(|blocks| blocks
                            .iter()
                            .any(|block| block["type"] == "tool_result"
                                && block["tool_use_id"] == "probe-1"
                                && block["content"]
                                    .as_str()
                                    .is_some_and(|text| text.contains("persisted_intent"))))
                    )
            );
        }
        bail!("scripted failure after committed tool result")
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".into())
    }
}

#[tokio::test]
async fn loop_commits_effects_before_followup_and_retains_progress_on_error() -> Result<()> {
    let (_temp, sessions, workspace, state) = test_runtime_storage();
    let session_id = "loop-checkpoint-order".to_string();
    let backend = Arc::new(CheckpointBackend {
        sessions: sessions.clone(),
        session_id: session_id.clone(),
        calls: AtomicUsize::new(0),
    });
    let mut tools = ToolManager::new();
    tools.register(Box::new(CheckpointTool {
        sessions: sessions.clone(),
        session_id: session_id.clone(),
    }));
    let mut agent = Agent::new(
        tools,
        backend.clone(),
        Arc::new(MemoryHandle::new(&state.join("memory").to_string_lossy())),
        sessions,
        workspace,
    );
    agent.session_id = session_id;
    agent.history.push(Message {
        role: "user".into(),
        content: json!("probe the checkpoint"),
    });
    let mut turns = 0;
    let mut events = Vec::new();
    let error = agent
        .run_agent_loop_with_limit(
            AgentOutputMode::Silent,
            &mut |event| events.push(event),
            &mut turns,
        )
        .await
        .expect_err("second request must fail");
    assert!(error.to_string().contains("scripted failure"));
    assert_eq!(turns, 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    assert!(events.iter().any(|event| matches!(event, AgentEvent::ToolResult { call_id, is_error: false, .. } if call_id == "probe-1")));
    agent.max_turns = Some(1);
    agent
        .run_agent_loop_with_limit(AgentOutputMode::Silent, &mut |_| {}, &mut turns)
        .await?;
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        agent.last_agent_turn_trace.continuation_phase.as_deref(),
        Some("max_turns_reached")
    );
    Ok(())
}
