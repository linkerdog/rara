use serde_json::json;

use super::LlmTurnMetadata;
use super::openai_compatible::{build_chat_completion_request_body, to_codex_input_items};
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
