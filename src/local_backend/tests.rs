use serde_json::json;

use super::model::{LocalModelSpec, default_local_model_cache_dir, extract_context_window};
use super::parser::{extract_json_object, parse_tool_aware_reply};
use super::prompt::{render_content, scenario_token_cap};
use crate::agent::Message;

#[test]
fn resolves_supported_aliases() {
    assert_eq!(
        LocalModelSpec::from_alias_or_model_id("gemma4-e2b")
            .unwrap()
            .model_id(),
        "google/gemma-4-E2B-it"
    );
    assert_eq!(
        LocalModelSpec::from_alias_or_model_id("qwen3-8b")
            .unwrap()
            .model_id(),
        "Qwen/Qwen3-8B"
    );
    assert_eq!(
        LocalModelSpec::from_alias_or_model_id("qwn3-8b")
            .unwrap()
            .model_id(),
        "Qwen/Qwen3-8B"
    );
}

#[test]
fn builds_global_cache_path() {
    let path = default_local_model_cache_dir();
    assert!(path.to_string_lossy().contains("rara"));
    assert!(path.to_string_lossy().contains("huggingface"));
}

#[test]
fn extracts_first_json_object_from_mixed_text() {
    let raw = "```json\n{\"kind\":\"final\",\"text\":\"ok\"}\n```";
    assert_eq!(
        extract_json_object(raw),
        Some("{\"kind\":\"final\",\"text\":\"ok\"}")
    );
}

#[test]
fn parses_tool_reply() {
    let raw = "{\"kind\":\"tool\",\"calls\":[{\"name\":\"read_file\",\"input\":{\"path\":\"Cargo.toml\"}}]}";
    let reply = parse_tool_aware_reply(raw).unwrap();
    assert_eq!(reply.kind.as_deref(), Some("tool"));
    assert_eq!(reply.calls.unwrap()[0].name, "read_file");
}

#[test]
fn renders_tool_results_for_prompting() {
    let rendered = render_content(&json!([
        {"type": "rara_model_context", "kind": "environment", "text": "workspace context"},
        {"type": "text", "text": "hello"},
        {"type": "tool_result", "tool_use_id": "1", "content": "world"}
    ]));
    assert!(rendered.contains("workspace context"));
    assert!(rendered.contains("hello"));
    assert!(rendered.contains("tool_result(id=1): world"));
}

#[test]
fn extracts_context_window_from_text_config() {
    let raw = json!({
        "text_config": {
            "max_position_embeddings": 32768
        }
    });
    assert_eq!(extract_context_window(&raw), Some(32768));
}

#[test]
fn uses_smaller_budget_for_short_and_tool_prompts() {
    let short_messages = vec![Message {
        role: "user".to_string(),
        content: json!([
            {
                "type": "rara_model_context",
                "kind": "environment",
                "text": "a deliberately long model-only environment context"
            },
            {"type": "text", "text": "你好"}
        ]),
    }];
    assert_eq!(scenario_token_cap(&short_messages, &[]), 96);

    let normal_messages = vec![Message {
        role: "user".to_string(),
        content: json!([{"type": "text", "text": "Explain this repository structure."}]),
    }];
    assert_eq!(
        scenario_token_cap(&normal_messages, &[json!({"name":"read_file"})]),
        128
    );
    assert_eq!(scenario_token_cap(&normal_messages, &[]), 192);
}
