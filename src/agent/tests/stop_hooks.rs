use std::fs;
use std::sync::Arc;

use rara_memory::vectordb::VectorDB;
use rara_tools::tool::ToolManager;
use serde_json::json;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentEvent, AgentOutputMode};
use crate::hooks::HookOutcome;
use crate::hooks::{HookRegistry, HookSandbox};
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

#[tokio::test]
async fn stop_hook_blocks_completion_and_returns_feedback_to_the_model() {
    let (temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let claude_dir = temp.path().join(".claude");
    fs::create_dir_all(&claude_dir).expect("mkdir");
    fs::write(
        claude_dir.join("settings.json"),
        r#"{
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "if [ -e .stop-hook-ran ]; then exit 0; fi; cat > .stop-hook-input.json; touch .stop-hook-ran; echo 'Run the visible completion check.' >&2; exit 2"
                    }]
                }]
            }
        }"#,
    )
    .expect("settings");
    let mut registry = HookRegistry::new();
    registry.discover_repo_hooks(temp.path());

    let backend = Arc::new(SequencedBackend::new(vec![
        response("first completion"),
        response("verified completion"),
    ]));
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.history.push(crate::agent::Message {
        role: "user".to_string(),
        content: json!("complete the task"),
    });
    agent.set_hook_context(
        Arc::new(registry),
        HookSandbox {
            workspace_root: temp.path().to_path_buf(),
            ..HookSandbox::default()
        },
    );

    let mut events = Vec::new();
    agent
        .run_agent_loop_with_limit(
            AgentOutputMode::Silent,
            &mut |event| events.push(event),
            &mut 0,
        )
        .await
        .expect("agent loop");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AgentError { message, recoverable: true }
            if message.contains("Run the visible completion check.")
    )));
    assert!(backend.observed_messages()[1].iter().any(|message| {
        message.role == "system"
            && message
                .content
                .as_str()
                .is_some_and(|content| content.contains("Stop hook"))
    }));
    let hook_input: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join(".stop-hook-input.json")).expect("hook input"),
    )
    .expect("valid hook input");
    assert_eq!(
        hook_input["last_assistant_message"],
        serde_json::Value::String("first completion".to_string())
    );
}

fn response(text: &str) -> LlmResponse {
    LlmResponse {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    }
}

#[test]
fn stop_hook_json_block_reason_is_returned_to_the_agent() {
    let outcome = HookOutcome {
        stdout: r#"{"decision":"block","reason":"Run the completion check."}"#.to_string(),
        stderr: String::new(),
        exit_code: Some(0),
        timed_out: false,
    };

    assert_eq!(
        super::super::stop_hook_block_reason(&outcome),
        Some("Run the completion check.".to_string())
    );
}

#[test]
fn message_text_extracts_text_blocks_from_structured_assistant_content() {
    let message = crate::agent::Message {
        role: "assistant".to_string(),
        content: json!([
            {"type": "text", "text": "first "},
            {"type": "tool_use", "name": "read_file"},
            {"type": "text", "text": "last"}
        ]),
    };

    assert_eq!(
        super::super::message_text(&message),
        Some("first last".to_string())
    );
}
