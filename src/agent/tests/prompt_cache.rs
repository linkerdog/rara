use std::sync::Arc;

use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentExecutionMode, AgentOutputMode};
use crate::llm::{ContentBlock, LlmResponse, ProviderCacheProfile, TokenUsage};

fn text_response(text: &str) -> LlmResponse {
    LlmResponse {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    }
}

#[tokio::test]
async fn later_request_preserves_the_previous_model_visible_prefix() {
    let backend = Arc::new(
        SequencedBackend::new(vec![
            text_response("first answer"),
            text_response("second answer"),
        ])
        .with_cache_profile(ProviderCacheProfile::automatic_prefix_cache_with_usage()),
    );
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
        .query_with_mode("first request".to_string(), AgentOutputMode::Silent)
        .await
        .expect("first query");
    agent.execution_mode = AgentExecutionMode::Plan;
    agent
        .query_with_mode("second request".to_string(), AgentOutputMode::Silent)
        .await
        .expect("second query");

    let observed = backend.observed_messages();
    let first = observed.first().expect("first model request");
    let second = observed.get(1).expect("second model request");
    assert_eq!(&second[..first.len()], first.as_slice());
    assert_eq!(first[0].role, "system");
    let system = first[0].content.as_str().expect("plain system prompt");
    assert!(!system.contains("__DYNAMIC_BOUNDARY__"));
    assert!(!system.contains("<environment_context>"));
    assert!(!system.contains("Current Execution Mode"));
    assert!(first[1].content.to_string().contains("environment"));
    assert!(first[1].content.to_string().contains("execution_mode"));
    assert!(
        second
            .last()
            .expect("latest user request")
            .content
            .to_string()
            .contains("Planning mode is active.")
    );
}
