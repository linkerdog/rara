use std::sync::Arc;

use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;
use serde_json::json;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentOutputMode, Message};
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

#[tokio::test]
async fn model_request_projects_old_tool_results_without_mutating_history() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "done".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
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

    agent.history = (0..8)
        .flat_map(|idx| {
            [
                Message {
                    role: "assistant".to_string(),
                    content: json!([{
                        "type": "tool_use",
                        "id": format!("tool-{idx}"),
                        "name": "read_file",
                        "input": { "path": format!("src/{idx}.rs") }
                    }]),
                },
                Message {
                    role: "user".to_string(),
                    content: json!([{
                        "type": "tool_result",
                        "tool_use_id": format!("tool-{idx}"),
                        "content": format!("old-result-{idx}\n{}", "x".repeat(12_000))
                    }]),
                },
            ]
        })
        .collect();
    agent.compact_state.estimated_history_tokens = 1;

    agent
        .query_with_mode("continue".to_string(), AgentOutputMode::Silent)
        .await
        .expect("query");

    let observed = backend.observed_messages();
    let request = observed.first().expect("first model request");
    let request_text = serde_json::to_string(request).expect("request json");
    assert!(request_text.contains("Old tool result content cleared"));
    assert!(!request_text.contains("old-result-0"));
    assert!(request_text.contains("old-result-7"));
    let runtime_context = agent.shared_runtime_context();
    assert!(runtime_context.observability.microcompact.cleared_results > 0);
    assert!(runtime_context.observability.microcompact.saved_chars > 0);
    assert_eq!(
        runtime_context.observability.microcompact.budget_chars,
        48_000
    );

    let persisted_history = serde_json::to_string(&agent.history).expect("history json");
    assert!(persisted_history.contains("old-result-0"));
    assert!(persisted_history.contains("old-result-7"));
}
