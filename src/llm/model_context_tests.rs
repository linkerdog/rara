use serde_json::json;

use super::LlmTurnMetadata;
use super::openai_compatible::{
    build_chat_completion_request_body, build_codex_responses_request, to_codex_input_items,
};
use crate::agent::Message;
use crate::config::OpenAiEndpointKind;

#[test]
fn deepseek_request_uses_plain_prefix_without_anthropic_cache_controls() {
    let messages = model_context_messages();

    let body = build_chat_completion_request_body(
        "deepseek-chat",
        &messages,
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        None,
        LlmTurnMetadata::execute(),
    );
    let serialized = body.to_string();

    assert_eq!(body["messages"][0]["content"], "stable system prompt");
    assert_eq!(
        body["messages"][1]["content"],
        "<environment_context><cwd>/workspace</cwd></environment_context>\n\ninspect the cache"
    );
    assert!(!serialized.contains("cache_control"));
    assert!(!serialized.contains("__DYNAMIC_BOUNDARY__"));
}

#[test]
fn codex_responses_renders_model_context_before_human_text() {
    let input = to_codex_input_items(&model_context_messages());

    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(
        input[0]["content"][0]["text"],
        "<environment_context><cwd>/workspace</cwd></environment_context>\n\ninspect the cache"
    );
}

#[test]
fn responses_append_preserves_instructions_tools_options_and_control_authority() {
    let mut manager = rara_tools::tool::ToolManager::new();
    manager.register(Box::<rara_tools::file::ReadFileTool>::default());
    manager.register(Box::<rara_tools::file::WriteFileTool>::default());
    let tools = manager.get_schemas();
    assert_eq!(tools.len(), 2);
    let mut messages = model_context_messages();
    messages.insert(
        1,
        Message {
            role: "system".into(),
            content: json!("stable workspace rules"),
        },
    );
    messages.push(Message { role: "assistant".into(), content: json!([
        {"type": "tool_use", "id": "read-1", "name": "read_file", "input": {"path": "Cargo.toml"}}
    ]) });
    messages.push(Message {
        role: "user".into(),
        content: json!([
            {"type": "tool_result", "tool_use_id": "read-1", "content": "[package]"}
        ]),
    });
    let first = build_codex_responses_request("gpt-5.4", &messages, &tools, Some("high")).unwrap();
    messages.push(Message {
        role: "system".into(),
        content: json!("Planning is now active; do not modify files."),
    });
    let second = build_codex_responses_request("gpt-5.4", &messages, &tools, Some("high")).unwrap();
    assert_eq!(
        first["instructions"],
        "stable system prompt\n\nstable workspace rules"
    );
    for field in [
        "instructions",
        "tools",
        "reasoning",
        "tool_choice",
        "parallel_tool_calls",
    ] {
        assert_eq!(first[field], second[field], "{field} changed after append");
    }
    let before = first["input"].as_array().unwrap();
    let after = second["input"].as_array().unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    assert_eq!(after.last().unwrap()["role"], "system");
    assert_eq!(after[before.len() - 1]["call_id"], "read-1");
    assert_eq!(after[before.len() - 1]["type"], "function_call_output");
}

fn model_context_messages() -> Vec<Message> {
    vec![
        Message {
            role: "system".to_string(),
            content: json!("stable system prompt"),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {
                    "type": "rara_model_context",
                    "kind": "environment",
                    "text": "<environment_context><cwd>/workspace</cwd></environment_context>"
                },
                {"type": "text", "text": "inspect the cache"}
            ]),
        },
    ]
}
