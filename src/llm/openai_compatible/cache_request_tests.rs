use super::*;
use crate::llm::AnthropicCacheTtl;

#[test]
fn advancing_checkpoint_includes_tool_results_without_changing_call_identity() {
    let model = "anthropic/claude-sonnet-4.6";
    let backend = OpenAiCompatibleBackend::new_with_endpoint_kind(
        None,
        "https://openrouter.ai/api/v1".into(),
        model.into(),
        OpenAiEndpointKind::Openrouter,
    )
    .unwrap();
    let tools = vec![
        json!({"name":"Read", "description":"Read a file", "input_schema":{
            "type":"object", "properties":{"path":{"type":"string"}}, "required":["path"]
        }}),
    ];
    let mut messages = vec![
        Message {
            role: "system".into(),
            content: json!("stable rules"),
        },
        Message {
            role: "user".into(),
            content: json!("inspect both files"),
        },
    ];
    let mut bodies = Vec::new();
    for id in ["read-1", "read-2"] {
        messages.push(Message {
            role: "assistant".into(),
            content: json!([
                {"type":"tool_use", "id":id, "name":"Read", "input":{"path":"file.txt"}}
            ]),
        });
        messages.push(Message {
            role: "user".into(),
            content: json!([
                {"type":"tool_result", "tool_use_id":id, "content":"file evidence"}
            ]),
        });
        let body = backend.chat_completion_request_body(
            model,
            &messages,
            &tools,
            LlmTurnMetadata::default(),
        );
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], "tool");
        assert_eq!(last["tool_call_id"], id);
        assert_eq!(last["content"][0]["text"], "file evidence");
        assert_eq!(last["content"][0]["cache_control"]["ttl"], "5m");
        bodies.push(body);
    }
    assert_eq!(bodies[0]["messages"][0], bodies[1]["messages"][0]);
    assert_eq!(bodies[0]["tools"], bodies[1]["tools"]);
    assert_eq!(bodies[1]["messages"][3]["tool_call_id"], "read-1");
    assert!(bodies[1]["messages"][3]["content"].is_string());
}

#[test]
fn production_builder_gates_anthropic_checkpoints_by_endpoint_and_model() {
    let messages = vec![
        Message {
            role: "system".into(),
            content: json!("stable instructions"),
        },
        Message {
            role: "user".into(),
            content: json!("first task"),
        },
    ];
    let tools = vec![
        json!({"name":"Read", "description":"Read a file", "input_schema":{
            "type":"object", "properties":{"path":{"type":"string"}}, "required":["path"]
        }}),
    ];
    for (endpoint, kind, model, supported) in [
        (
            "https://openrouter.ai/api/v1",
            OpenAiEndpointKind::Openrouter,
            "anthropic/claude-sonnet-4.6",
            true,
        ),
        (
            "https://gateway.example/v1",
            OpenAiEndpointKind::Openrouter,
            "anthropic/claude-sonnet-4.6",
            false,
        ),
        (
            "https://openrouter.ai/api/v1",
            OpenAiEndpointKind::Openrouter,
            "openrouter/auto",
            false,
        ),
        (
            "https://api.deepseek.com",
            OpenAiEndpointKind::Deepseek,
            "deepseek-v4-flash",
            false,
        ),
        (
            "https://api.openai.com/v1",
            OpenAiEndpointKind::Custom,
            "gpt-5.4",
            false,
        ),
    ] {
        let backend = OpenAiCompatibleBackend::new_with_endpoint_kind(
            None,
            endpoint.into(),
            model.into(),
            kind,
        )
        .expect("fixture backend");
        let first = backend.chat_completion_request_body(
            model,
            &messages,
            &tools,
            LlmTurnMetadata::default(),
        );
        assert!(!first["tools"].as_array().unwrap().is_empty());
        assert!(first.get("cache_control").is_none());
        if supported {
            assert_eq!(
                first["messages"][0]["content"][0]["cache_control"]["ttl"],
                "5m"
            );
            let backend = backend.with_anthropic_cache_ttl(AnthropicCacheTtl::OneHour);
            let mut next_messages = messages.clone();
            next_messages.push(Message {
                role: "user".into(),
                content: json!("second task"),
            });
            let next = backend.chat_completion_request_body(
                model,
                &next_messages,
                &tools,
                LlmTurnMetadata::default(),
            );
            assert_eq!(first["tools"], next["tools"]);
            assert_eq!(
                next["messages"][0]["content"][0]["cache_control"]["ttl"],
                "1h"
            );
            assert!(next["messages"][1]["content"].is_string());
            assert_eq!(
                next["messages"][2]["content"][0]["cache_control"]["ttl"],
                "1h"
            );
        } else {
            assert!(
                !first.to_string().contains("cache_control"),
                "{endpoint} {model}"
            );
        }
    }
}
