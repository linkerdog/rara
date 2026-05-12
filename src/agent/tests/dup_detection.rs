use std::sync::Arc;

use rara_memory::vectordb::VectorDB;
use rara_tools::tool::ToolManager;
use serde_json::json;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentEvent, AgentOutputMode, Message};
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

#[tokio::test]
async fn repeated_identical_tool_calls_emit_status_warning() {
    // Backend returns two identical tool_use blocks, then end_turn
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![
                ContentBlock::ToolUse {
                    id: "tool-1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "foo.txt"}),
                },
                ContentBlock::ToolUse {
                    id: "tool-2".into(),
                    name: "read_file".into(),
                    input: json!({"path": "foo.txt"}),
                },
            ],
            stop_reason: Some("tool_use".into()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            stop_reason: Some("end_turn".into()),
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

    // Seed the conversation with a user message
    agent.history = vec![Message {
        role: "user".into(),
        content: json!("read foo.txt twice"),
    }];

    let mut status_messages: Vec<String> = Vec::new();
    let mut report = |event: AgentEvent| {
        if let AgentEvent::Status(msg) = &event {
            status_messages.push(msg.clone());
        }
    };

    // Run the loop — the first response has 2 tool calls,
    // the second is end_turn. The primary agent loop will
    // try to dispatch tools, fail, and continue.
    let _ = agent
        .run_agent_loop_with_limit(AgentOutputMode::Silent, &mut report, &mut 0usize)
        .await;

    let has_dup_warning = status_messages
        .iter()
        .any(|m| m.contains("Repeated tool call"));
    // The detection code should fire because both tool calls
    // in the first response are identical
    assert!(
        has_dup_warning,
        "should emit duplicate tool call warning, got: {:?}",
        status_messages
    );
}
