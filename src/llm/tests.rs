use reqwest::StatusCode;
use serde_json::json;

use super::ollama::{
    apply_ollama_stream_event, build_ollama_options, ensure_ollama_stream_completed,
    suggest_ollama_num_ctx, to_ollama_messages,
};
use super::openai_compatible::{
    DeepseekTextStreamScrubber, OpenAiApiError, apply_codex_stream_event,
    build_chat_completion_request_body, build_codex_responses_request,
    build_streaming_response_content, infer_openai_compatible_auxiliary_model,
    is_auxiliary_model_retryable_error, is_auxiliary_model_unsupported_error,
    is_context_window_error, is_openai_stream_idle_error, merge_streaming_tool_calls,
    parse_chat_completion_response, parse_codex_response, to_codex_input_items, to_openai_messages,
    to_openai_messages_for_endpoint,
};
use super::shared::{
    extract_message_text, model_context_budget, parse_tool_arguments, should_bypass_proxy,
};
use crate::agent::Message;
use crate::config::OpenAiEndpointKind;
use crate::llm::{ContentBlock, LlmStreamEvent, LlmTurnMetadata};
use crate::tool::Tool;
use crate::tools::planning::ExitPlanModeTool;

#[test]
fn converts_assistant_tool_history_to_openai_messages() {
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"text","text":"Need a tool."},
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"Cargo.toml"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"[package]"}
            ]),
        },
    ];

    let openai_messages = to_openai_messages(&messages);
    assert_eq!(openai_messages[0]["role"], "assistant");
    assert_eq!(
        openai_messages[0]["tool_calls"][0]["function"]["name"],
        "read_file"
    );
    assert_eq!(openai_messages[1]["role"], "tool");
    assert_eq!(openai_messages[1]["tool_call_id"], "tool-1");
}

#[test]
fn converts_multiple_tool_results_to_adjacent_openai_tool_messages() {
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"Cargo.toml"}},
                {"type":"tool_use","id":"tool-2","name":"list_files","input":{"path":"src"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"[package]"},
                {"type":"tool_result","tool_use_id":"tool-2","content":"src/main.rs"}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!("<agent_runtime>\nphase: tool_results_available\n</agent_runtime>"),
        },
    ];

    let openai_messages = to_openai_messages(&messages);
    assert_eq!(openai_messages[0]["role"], "assistant");
    assert_eq!(openai_messages[1]["role"], "tool");
    assert_eq!(openai_messages[1]["tool_call_id"], "tool-1");
    assert_eq!(openai_messages[2]["role"], "tool");
    assert_eq!(openai_messages[2]["tool_call_id"], "tool-2");
    assert_eq!(openai_messages[3]["role"], "user");
    assert_eq!(
        openai_messages[3]["content"],
        "<agent_runtime>\nphase: tool_results_available\n</agent_runtime>"
    );
}

#[test]
fn fills_missing_openai_tool_results_before_follow_up_messages() {
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"Cargo.toml"}},
                {"type":"tool_use","id":"tool-2","name":"list_files","input":{"path":"src"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"[package]"}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!("continue"),
        },
    ];

    let openai_messages = to_openai_messages(&messages);
    assert_eq!(openai_messages[0]["role"], "assistant");
    assert_eq!(openai_messages[1]["role"], "tool");
    assert_eq!(openai_messages[1]["tool_call_id"], "tool-1");
    assert_eq!(openai_messages[2]["role"], "tool");
    assert_eq!(openai_messages[2]["tool_call_id"], "tool-2");
    assert!(
        openai_messages[2]["content"]
            .as_str()
            .is_some_and(|content| content.contains("interrupted before a result was recorded"))
    );
    assert_eq!(openai_messages[3]["role"], "user");
    assert_eq!(openai_messages[3]["content"], "continue");
}

#[test]
fn drops_orphan_openai_tool_results_without_context_prefix() {
    let messages = vec![
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"orphan","content":"stale result"}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"text","text":"continue"},
                {"type":"tool_result","tool_use_id":"orphan","content":"stale result"}
            ]),
        },
    ];

    let openai_messages = to_openai_messages(&messages);
    assert_eq!(openai_messages.len(), 1);
    assert_eq!(openai_messages[0]["role"], "user");
    assert_eq!(openai_messages[0]["content"], "continue");
    assert!(!openai_messages.iter().any(|message| {
        message["content"]
            .as_str()
            .is_some_and(|content| content.contains("tool_result orphan:"))
    }));
}

#[test]
fn skips_invalid_internal_tool_uses_when_rendering_openai_history() {
    let messages = vec![Message {
        role: "assistant".to_string(),
        content: json!([
            {"type":"tool_use","id":"","name":"read_file","input":{"path":"Cargo.toml"}},
            {"type":"tool_use","id":"tool-1","name":"","input":{"path":"src"}},
            {"type":"text","text":"Need more context."}
        ]),
    }];

    let openai_messages = to_openai_messages(&messages);
    assert_eq!(openai_messages.len(), 1);
    assert_eq!(openai_messages[0]["role"], "assistant");
    assert!(openai_messages[0].get("tool_calls").is_none());
    assert_eq!(openai_messages[0]["content"], "Need more context.");
}

#[test]
fn rejects_openai_tool_calls_without_required_fields() {
    let error = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        OpenAiEndpointKind::Deepseek,
    )
    .expect_err("missing tool call id should fail");

    assert!(error.to_string().contains("tool_calls[0] missing id"));
}

#[test]
fn deepseek_reasoning_content_roundtrips_as_provider_metadata() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning_content": "private chain summary",
                    "content": "Visible answer"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 3,
                "completion_tokens": 4
            }
        }),
        OpenAiEndpointKind::Deepseek,
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 2);
    assert!(matches!(
        &response.content[0],
        ContentBlock::Text { text } if text == "Visible answer"
    ));
    assert!(matches!(
        &response.content[1],
        ContentBlock::ProviderMetadata { provider, key, value }
            if provider == "deepseek"
                && key == "reasoning_content"
                && value == "private chain summary"
    ));

    let messages = vec![Message {
        role: "assistant".to_string(),
        content: serde_json::to_value(&response.content).expect("content json"),
    }];
    let deepseek_messages =
        to_openai_messages_for_endpoint(&messages, OpenAiEndpointKind::Deepseek);
    assert_eq!(
        deepseek_messages[0]["reasoning_content"],
        "private chain summary"
    );
    assert_eq!(deepseek_messages[0]["content"], "Visible answer");

    let generic_messages = to_openai_messages(&messages);
    assert!(generic_messages[0].get("reasoning_content").is_none());
}

#[test]
fn deepseek_raw_leading_think_block_is_not_visible_text() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "<think>private reasoning</think>\nVisible answer.<｜end▁of▁sentence｜>"
                },
                "finish_reason": "stop"
            }]
        }),
        OpenAiEndpointKind::Deepseek,
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 1);
    assert!(matches!(
        &response.content[0],
        ContentBlock::Text { text }
            if text.trim() == "Visible answer." && !text.contains("private reasoning")
    ));
}

#[test]
fn deepseek_endpoint_preserves_literal_leading_think_without_control_evidence() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "<think>inner</think> is an XML example."
                },
                "finish_reason": "stop"
            }]
        }),
        OpenAiEndpointKind::Deepseek,
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 1);
    assert!(matches!(
        &response.content[0],
        ContentBlock::Text { text } if text == "<think>inner</think> is an XML example."
    ));
}

#[test]
fn generic_openai_compatible_endpoint_preserves_literal_leading_think_text() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "<think>inner</think> is an XML example."
                },
                "finish_reason": "stop"
            }]
        }),
        OpenAiEndpointKind::Custom,
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 1);
    assert!(matches!(
        &response.content[0],
        ContentBlock::Text { text } if text == "<think>inner</think> is an XML example."
    ));
}

#[test]
fn openai_usage_tracks_cache_hit_and_miss_tokens() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Visible answer"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 4,
                "prompt_tokens_details": {
                    "cached_tokens": 64
                }
            }
        }),
        OpenAiEndpointKind::Kimi,
    )
    .expect("parse response");

    let usage = response.usage.expect("usage");
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 4);
    assert_eq!(usage.cache_hit_tokens, 64);
    assert_eq!(usage.cache_miss_tokens, 36);
}

#[test]
fn deepseek_skips_reasoning_only_assistant_history() {
    let messages = vec![
        Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"List your todo."}]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"provider_metadata","provider":"deepseek","key":"reasoning_content","value":"internal planning only"}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"Continue."}]),
        },
    ];

    let deepseek_messages =
        to_openai_messages_for_endpoint(&messages, OpenAiEndpointKind::Deepseek);

    assert_eq!(deepseek_messages.len(), 2);
    assert!(deepseek_messages.iter().all(|message| {
        !(message["role"] == "assistant"
            && message.get("tool_calls").is_none()
            && message
                .get("content")
                .is_none_or(|content| content.is_null()))
    }));
    assert_eq!(deepseek_messages[0]["role"], "user");
    assert_eq!(deepseek_messages[1]["role"], "user");
}

#[test]
fn deepseek_tool_call_reasoning_content_roundtrips_without_trimming() {
    let reasoning_content = "\n private chain summary \n";
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning_content": reasoning_content,
                    "content": "",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"Cargo.toml\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        OpenAiEndpointKind::Deepseek,
    )
    .expect("parse response");

    assert!(matches!(
        &response.content[0],
        ContentBlock::ProviderMetadata { provider, key, value }
            if provider == "deepseek"
                && key == "reasoning_content"
                && value == reasoning_content
    ));
    assert!(matches!(
        &response.content[1],
        ContentBlock::ToolUse { id, name, input }
            if id == "call-1" && name == "read_file" && input == &json!({"path":"Cargo.toml"})
    ));

    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: serde_json::to_value(&response.content).expect("content json"),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"call-1","content":"[package]"}
            ]),
        },
    ];
    let openai_messages = to_openai_messages_for_endpoint(&messages, OpenAiEndpointKind::Deepseek);

    assert_eq!(openai_messages[0]["reasoning_content"], reasoning_content);
    assert_eq!(openai_messages[0]["tool_calls"][0]["id"], "call-1");
    assert_eq!(openai_messages[1]["role"], "tool");
    assert_eq!(openai_messages[1]["tool_call_id"], "call-1");
}

#[test]
fn deepseek_streaming_reasoning_content_preserves_exact_bytes() {
    let reasoning_content = " \n private chain summary \n ";
    let content = build_streaming_response_content(
        OpenAiEndpointKind::Deepseek,
        "Visible answer".to_string(),
        reasoning_content.to_string(),
        &[],
    )
    .expect("build streaming content");

    assert_eq!(content.len(), 2);
    assert!(matches!(
        &content[0],
        ContentBlock::Text { text } if text == "Visible answer"
    ));
    assert!(matches!(
        &content[1],
        ContentBlock::ProviderMetadata { provider, key, value }
            if provider == "deepseek"
                && key == "reasoning_content"
                && value == reasoning_content
    ));
}

#[test]
fn deepseek_reasoner_defaults_preserve_standard_body() {
    let body = build_chat_completion_request_body(
        "deepseek-reasoner",
        &[Message {
            role: "user".to_string(),
            content: json!("Inspect the repository."),
        }],
        &[json!({
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type":"object"}
        })],
        OpenAiEndpointKind::Deepseek,
        None,
        None,
        LlmTurnMetadata::default(),
    );

    assert!(body.get("thinking").is_none());
    assert!(body.get("reasoning_effort").is_none());
    assert!(
        body["tools"]
            .as_array()
            .is_some_and(|tools| tools.len() == 1)
    );
}

#[test]
fn deepseek_v4_pro_infers_lite_auxiliary_model() {
    assert_eq!(
        infer_openai_compatible_auxiliary_model("deepseek-v4-pro", OpenAiEndpointKind::Deepseek)
            .as_deref(),
        Some("deepseek-v4-lite")
    );
    assert_eq!(
        infer_openai_compatible_auxiliary_model("DeepSeek-V4-PRO", OpenAiEndpointKind::Deepseek)
            .as_deref(),
        Some("DeepSeek-V4-lite")
    );
    assert_eq!(
        infer_openai_compatible_auxiliary_model("deepseek-v4-lite", OpenAiEndpointKind::Deepseek),
        None
    );
    assert_eq!(
        infer_openai_compatible_auxiliary_model("deepseek-v4-pro", OpenAiEndpointKind::Custom),
        None
    );
}

#[test]
fn auxiliary_model_fallback_is_limited_to_model_support_errors() {
    assert!(is_auxiliary_model_unsupported_error(&anyhow::Error::new(
        OpenAiApiError::for_test(
            Some(StatusCode::NOT_FOUND),
            Some("invalid_request_error"),
            None
        )
    )));
    assert!(is_auxiliary_model_unsupported_error(&anyhow::Error::new(
        OpenAiApiError::for_test(
            Some(StatusCode::BAD_REQUEST),
            Some("invalid_request_error"),
            Some("model_not_supported")
        )
    )));
    assert!(!is_auxiliary_model_unsupported_error(&anyhow::anyhow!(
        "API Error: rate limit exceeded"
    )));
    assert!(!is_auxiliary_model_unsupported_error(&anyhow::Error::new(
        OpenAiApiError::for_test(
            Some(StatusCode::UNAUTHORIZED),
            Some("invalid_api_key"),
            None
        )
    )));
}

#[test]
fn auxiliary_model_retry_uses_structured_context_window_code() {
    let error = anyhow::Error::new(OpenAiApiError::from_response_failed_error(&json!({
        "code": "context_length_exceeded",
        "message": "Your input exceeds the context window of this model."
    })));

    assert!(is_context_window_error(&error));
    assert!(is_auxiliary_model_retryable_error(&error));
    assert!(!is_context_window_error(&anyhow::anyhow!(
        "Your input exceeds the context window of this model."
    )));
}

#[test]
fn deepseek_v4_defaults_fold_legacy_tool_results_without_reasoning_content() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"text","text":""},
                    {"type":"tool_use","id":"tool-1","name":"bash","input":{"command":"cargo check"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"ok"}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{
                    "type": "text",
                    "text": "<agent_runtime>\n{\"phase\":\"tool_results_available\"}\n</agent_runtime>"
                }]),
            },
        ],
        &[json!({
            "name": "bash",
            "description": "Run shell command",
            "input_schema": {"type":"object"}
        })],
        OpenAiEndpointKind::Deepseek,
        None,
        None,
        LlmTurnMetadata::default(),
    );

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    let folded_note = messages[0]["content"].as_str().expect("folded note");
    assert!(folded_note.contains("<rara_internal_history_context>"));
    assert!(folded_note.contains("historical tool request: name=bash"));
    assert!(folded_note.contains("id=tool-1"));
    assert!(folded_note.contains("cargo check"));
    assert!(folded_note.contains("tool: ok"));
    assert!(!folded_note.contains("<agent_runtime>"));
    assert!(!folded_note.contains("tool_call:"));
    assert_eq!(messages[1]["role"], "user");
    assert!(
        messages[1]["content"]
            .as_str()
            .is_some_and(|content| content.contains("tool_results_available"))
    );
}

#[test]
fn deepseek_folded_legacy_history_excludes_runtime_control_messages() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"Cargo.toml"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"[package]"}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{
                    "type": "text",
                    "text": "<agent_runtime>\n{\"phase\":\"tool_results_available\"}\n</agent_runtime>"
                }]),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"text","text":"Now inspect the status file."},
                    {"type":"tool_use","id":"tool-2","name":"read_file","input":{"path":"src/tui/status_display.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-2","content":"status content"}
                ]),
            },
        ],
        &[json!({
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type":"object"}
        })],
        OpenAiEndpointKind::Deepseek,
        None,
        None,
        LlmTurnMetadata::default(),
    );

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    let folded_note = messages[0]["content"].as_str().expect("folded note");
    assert!(folded_note.contains("historical tool request: name=read_file"));
    assert!(folded_note.contains("src/tui/status_display.rs"));
    assert!(folded_note.contains("status content"));
    assert!(!folded_note.contains("<agent_runtime>"));
    assert!(!folded_note.contains("tool_results_available"));
    assert!(!folded_note.contains("tool_call:"));
}

#[test]
fn deepseek_thinking_tool_history_folds_missing_reasoning_content() {
    let body = build_chat_completion_request_body(
        "deepseek-reasoner",
        &[
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"text","text":""},
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"Cargo.toml"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"[package]"}
                ]),
            },
        ],
        &[json!({
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type":"object"}
        })],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::default(),
    );

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let folded_note = messages[0]["content"].as_str().expect("folded note");
    assert!(folded_note.contains("context only"));
    assert!(folded_note.contains("<rara_internal_history_context>"));
    assert!(folded_note.contains("historical tool request: name=read_file"));
    assert!(folded_note.contains("id=tool-1"));
    assert!(folded_note.contains("Cargo.toml"));
    assert!(folded_note.contains("[package]"));
    assert!(!folded_note.contains("tool_call:"));
}

#[test]
fn deepseek_thinking_tool_history_preserves_existing_reasoning_content() {
    let body = build_chat_completion_request_body(
        "deepseek-reasoner",
        &[Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"provider_metadata","provider":"deepseek","key":"reasoning_content","value":"keep this reasoning"},
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"Cargo.toml"}}
            ]),
        }],
        &[json!({
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type":"object"}
        })],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::default(),
    );

    assert_eq!(
        body["messages"][0]["reasoning_content"],
        "keep this reasoning"
    );
}

#[test]
fn deepseek_reasoner_explicit_thinking_enables_controls_for_tools() {
    let body = build_chat_completion_request_body(
        "deepseek-reasoner",
        &[Message {
            role: "user".to_string(),
            content: json!("Inspect the repository."),
        }],
        &[json!({
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type":"object"}
        })],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::default(),
    );

    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["reasoning_effort"], "max");
    assert!(
        body["tools"]
            .as_array()
            .is_some_and(|tools| tools.len() == 1)
    );
}

#[test]
fn deepseek_v4_explicit_thinking_enables_controls_for_tools() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[Message {
            role: "user".to_string(),
            content: json!("Inspect the repository."),
        }],
        &[json!({
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type":"object"}
        })],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::default(),
    );

    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["reasoning_effort"], "max");
    assert!(
        body["tools"]
            .as_array()
            .is_some_and(|tools| tools.len() == 1)
    );
}

#[test]
fn openai_chat_tools_enable_strict_for_structured_plan_schema() {
    let tool = ExitPlanModeTool;
    let body = build_chat_completion_request_body(
        "gpt-4o",
        &[Message {
            role: "user".to_string(),
            content: json!("Plan the implementation."),
        }],
        &[json!({
            "name": tool.name(),
            "description": tool.description(),
            "input_schema": tool.input_schema()
        })],
        OpenAiEndpointKind::Custom,
        None,
        None,
        LlmTurnMetadata::plan(),
    );

    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "exit_plan_mode");
    assert_eq!(body["tools"][0]["function"]["strict"], true);
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["properties"]["proposed_plan"]["required"],
        json!(["summary", "steps", "validation"])
    );
}

#[test]
fn openai_chat_tools_skip_strict_when_nested_object_is_not_strict_compatible() {
    let body = build_chat_completion_request_body(
        "gpt-4o",
        &[Message {
            role: "user".to_string(),
            content: json!("Plan the implementation."),
        }],
        &[json!({
            "name": "nested_tool",
            "description": "Tool with a nested object schema",
            "input_schema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "payload": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" }
                        }
                    }
                }
            }
        })],
        OpenAiEndpointKind::Custom,
        None,
        None,
        LlmTurnMetadata::default(),
    );

    assert_eq!(body["tools"][0]["type"], "function");
    assert!(body["tools"][0]["function"].get("strict").is_none());
}

#[test]
fn deepseek_reasoner_plan_with_explicit_thinking_uses_max_effort() {
    let body = build_chat_completion_request_body(
        "deepseek-reasoner",
        &[
            Message {
                role: "system".to_string(),
                content: json!("Custom prompt without plan-mode prose."),
            },
            Message {
                role: "user".to_string(),
                content: json!("Plan the implementation."),
            },
        ],
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::plan(),
    );

    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["reasoning_effort"], "max");
    assert!(body.get("tools").is_none());
}

#[test]
fn deepseek_explicit_thinking_folds_legacy_assistant_history_without_reasoning() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_value(vec![ContentBlock::Text {
                    text: "Legacy answer without provider metadata.".to_string(),
                }])
                .expect("content"),
            },
            Message {
                role: "user".to_string(),
                content: json!("Continue."),
            },
        ],
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::default(),
    );

    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["reasoning_effort"], "high");
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert!(
        messages[0]["content"]
            .as_str()
            .expect("folded note")
            .contains("Legacy answer without provider metadata.")
    );
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "Continue.");
}

#[test]
fn deepseek_folds_legacy_tool_calls_with_id_and_arguments() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_value(vec![ContentBlock::ToolUse {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "Cargo.toml"}),
                }])
                .expect("content"),
            },
            Message {
                role: "user".to_string(),
                content: json!("Continue."),
            },
        ],
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::default(),
    );

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages[0]["role"], "user");
    let folded_note = messages[0]["content"].as_str().expect("folded note");
    assert!(folded_note.contains("historical tool request: name=read_file"));
    assert!(folded_note.contains("id=call-1"));
    assert!(folded_note.contains("Cargo.toml"));
    assert!(!folded_note.contains("tool_call:"));
}

#[test]
fn deepseek_default_thinking_folds_legacy_assistant_history_without_reasoning() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_value(vec![ContentBlock::Text {
                    text: "Legacy answer without provider metadata.".to_string(),
                }])
                .expect("content"),
            },
            Message {
                role: "user".to_string(),
                content: json!("Continue."),
            },
        ],
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        None,
        LlmTurnMetadata::default(),
    );

    assert!(body.get("thinking").is_none());
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert!(
        messages[0]["content"]
            .as_str()
            .expect("folded note")
            .contains("Legacy answer without provider metadata.")
    );
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "Continue.");
}

#[test]
fn deepseek_disabled_thinking_preserves_legacy_assistant_history_without_reasoning() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_value(vec![ContentBlock::Text {
                    text: "Legacy answer without provider metadata.".to_string(),
                }])
                .expect("content"),
            },
            Message {
                role: "user".to_string(),
                content: json!("Continue."),
            },
        ],
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(false),
        LlmTurnMetadata::default(),
    );

    assert_eq!(body["thinking"]["type"], "disabled");
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(
        messages[0]["content"],
        "Legacy answer without provider metadata."
    );
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "Continue.");
}

#[test]
fn deepseek_explicit_thinking_stays_enabled_for_reasoning_compatible_history() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_value(vec![
                    ContentBlock::Text {
                        text: "Visible answer.".to_string(),
                    },
                    ContentBlock::ProviderMetadata {
                        provider: "deepseek".to_string(),
                        key: "reasoning_content".to_string(),
                        value: json!("preserved reasoning"),
                    },
                ])
                .expect("content"),
            },
            Message {
                role: "user".to_string(),
                content: json!("Continue."),
            },
        ],
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::default(),
    );

    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["reasoning_effort"], "high");
}

#[test]
fn deepseek_explicit_thinking_keeps_compatible_suffix_after_legacy_prefix() {
    let body = build_chat_completion_request_body(
        "deepseek-v4-pro",
        &[
            Message {
                role: "system".to_string(),
                content: json!("System prompt."),
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_value(vec![ContentBlock::Text {
                    text: "Legacy answer.".to_string(),
                }])
                .expect("content"),
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::to_value(vec![
                    ContentBlock::Text {
                        text: "Fresh answer.".to_string(),
                    },
                    ContentBlock::ProviderMetadata {
                        provider: "deepseek".to_string(),
                        key: "reasoning_content".to_string(),
                        value: json!("fresh reasoning"),
                    },
                ])
                .expect("content"),
            },
            Message {
                role: "user".to_string(),
                content: json!("Continue."),
            },
        ],
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        Some(true),
        LlmTurnMetadata::default(),
    );

    assert_eq!(body["thinking"]["type"], "enabled");
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "System prompt.");
    assert_eq!(messages[1]["role"], "user");
    assert!(
        messages[1]["content"]
            .as_str()
            .expect("folded note")
            .contains("Legacy answer.")
    );
    assert_eq!(messages[2]["content"], "Fresh answer.");
    assert_eq!(messages[2]["reasoning_content"], "fresh reasoning");
    assert_eq!(messages[3]["content"], "Continue.");
}

#[test]
fn deepseek_reasoner_explicit_thinking_normalizes_reasoning_effort() {
    let medium_body = build_chat_completion_request_body(
        "deepseek-reasoner",
        &[Message {
            role: "user".to_string(),
            content: json!("Explain this code."),
        }],
        &[],
        OpenAiEndpointKind::Deepseek,
        Some("medium"),
        Some(true),
        LlmTurnMetadata::default(),
    );
    let xhigh_body = build_chat_completion_request_body(
        "deepseek-reasoner",
        &[Message {
            role: "user".to_string(),
            content: json!("Explain this code."),
        }],
        &[],
        OpenAiEndpointKind::Deepseek,
        Some("xhigh"),
        Some(true),
        LlmTurnMetadata::default(),
    );

    assert_eq!(medium_body["reasoning_effort"], "high");
    assert_eq!(xhigh_body["reasoning_effort"], "max");
}

#[test]
fn deepseek_streaming_raw_leading_think_block_is_not_visible_text() {
    let content = build_streaming_response_content(
        OpenAiEndpointKind::Deepseek,
        "<think>private reasoning</think>\nVisible answer.<｜end▁of▁sentence｜>".to_string(),
        String::new(),
        &[],
    )
    .expect("build streaming content");

    assert_eq!(content.len(), 1);
    assert!(matches!(
        &content[0],
        ContentBlock::Text { text }
            if text.trim() == "Visible answer." && !text.contains("private reasoning")
    ));
}

#[test]
fn deepseek_streaming_preserves_literal_leading_think_without_control_evidence() {
    let content = build_streaming_response_content(
        OpenAiEndpointKind::Deepseek,
        "<think>inner</think> is an XML example.".to_string(),
        String::new(),
        &[],
    )
    .expect("build streaming content");

    assert_eq!(content.len(), 1);
    assert!(matches!(
        &content[0],
        ContentBlock::Text { text } if text == "<think>inner</think> is an XML example."
    ));
}

#[test]
fn deepseek_stream_scrubber_buffers_literal_leading_think_until_finish() {
    let mut scrubber = DeepseekTextStreamScrubber::default();

    assert_eq!(scrubber.push("<think>"), "");
    assert_eq!(scrubber.push("inner</think> is an XML example."), "");
    assert_eq!(scrubber.finish(), "<think>inner</think> is an XML example.");
}

#[test]
fn deepseek_stream_scrubber_strips_leading_think_when_control_evidence_arrives() {
    let mut scrubber = DeepseekTextStreamScrubber::default();

    assert_eq!(scrubber.push("<think>private"), "");
    assert_eq!(
        scrubber.push(" reasoning</think>\nVisible answer.<｜end▁of▁sentence｜>"),
        ""
    );
    assert_eq!(scrubber.finish().trim(), "Visible answer.");
}

#[test]
fn deepseek_non_thinking_model_keeps_standard_openai_body() {
    let body = build_chat_completion_request_body(
        "deepseek-chat",
        &[Message {
            role: "user".to_string(),
            content: json!("Hello"),
        }],
        &[],
        OpenAiEndpointKind::Deepseek,
        None,
        None,
        LlmTurnMetadata::default(),
    );

    assert!(body.get("thinking").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn parses_dsml_tool_calls_from_text_content() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": concat!(
                        "<｜DSML｜tool_calls>\n",
                        "<｜DSML｜invoke name=\"apply_patch\">\n",
                        "<｜DSML｜parameter name=\"patch\" string=\"true\">*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch</｜DSML｜parameter>\n",
                        "</｜DSML｜invoke>\n",
                        "</｜DSML｜tool_calls>"
                    )
                },
                "finish_reason": "tool_calls"
            }]
        }),
        OpenAiEndpointKind::Deepseek,
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 1);
    assert!(matches!(
        &response.content[0],
        ContentBlock::ToolUse { id, name, input }
            if id == "dsml-tool-1"
                && name == "apply_patch"
                && input["patch"].as_str().is_some_and(|patch| patch.contains("*** Begin Patch"))
    ));
}

#[test]
fn parses_visible_text_before_dsml_tool_calls_for_deepseek() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": concat!(
                        "I will inspect the file first.\n",
                        "<｜DSML｜tool_calls>\n",
                        "<｜DSML｜invoke name=\"read_file\">\n",
                        "<｜DSML｜parameter name=\"path\" string=\"true\">Cargo.toml</｜DSML｜parameter>\n",
                        "</｜DSML｜invoke>\n",
                        "</｜DSML｜tool_calls>"
                    )
                },
                "finish_reason": "stop"
            }]
        }),
        OpenAiEndpointKind::Deepseek,
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 2);
    assert!(matches!(
        &response.content[0],
        ContentBlock::Text { text } if text.contains("inspect the file")
    ));
    assert!(matches!(
        &response.content[1],
        ContentBlock::ToolUse { id, name, input }
            if id == "dsml-tool-1"
                && name == "read_file"
                && input["path"] == "Cargo.toml"
    ));
}

#[test]
fn parses_ascii_pipe_dsml_tool_calls_for_deepseek_pdf_compatibility() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": concat!(
                        "I will inspect the directory first.\n",
                        "<|DSML|tool_calls>\n",
                        "<|DSML|invoke name=\"list_files\">\n",
                        "<|DSML|parameter name=\"path\" string=\"true\">src</|DSML|parameter>\n",
                        "</|DSML|invoke>\n",
                        "</|DSML|tool_calls>"
                    )
                },
                "finish_reason": "tool_calls"
            }]
        }),
        OpenAiEndpointKind::Deepseek,
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 2);
    assert!(matches!(
        &response.content[0],
        ContentBlock::Text { text } if text.contains("inspect the directory")
    ));
    assert!(matches!(
        &response.content[1],
        ContentBlock::ToolUse { id, name, input }
            if id == "dsml-tool-1" && name == "list_files" && input["path"] == "src"
    ));
}

#[test]
fn ignores_dsml_tool_calls_for_generic_openai_compatible_endpoint() {
    let response = parse_chat_completion_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": concat!(
                        "Visible text\n",
                        "<｜DSML｜tool_calls>\n",
                        "<｜DSML｜invoke name=\"apply_patch\">\n",
                        "<｜DSML｜parameter name=\"patch\" string=\"true\">*** Begin Patch\n*** End Patch</｜DSML｜parameter>\n",
                        "</｜DSML｜invoke>\n",
                        "</｜DSML｜tool_calls>"
                    )
                },
                "finish_reason": "stop"
            }]
        }),
        OpenAiEndpointKind::Custom,
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 1);
    assert!(matches!(
        &response.content[0],
        ContentBlock::Text { text }
            if text.contains("Visible text") && text.contains("<｜DSML｜tool_calls>")
    ));
}

#[test]
fn merge_streaming_tool_calls_initializes_function_object() {
    let mut calls = Vec::new();

    merge_streaming_tool_calls(
        &mut calls,
        &[json!({
            "index": 0,
            "id": "call-1",
            "type": "function"
        })],
    )
    .expect("merge initial tool call");
    merge_streaming_tool_calls(
        &mut calls,
        &[json!({
            "index": 0,
            "function": {
                "name": "read_file",
                "arguments": "{\"path\":"
            }
        })],
    )
    .expect("merge tool call name and arguments");
    merge_streaming_tool_calls(
        &mut calls,
        &[json!({
            "index": 0,
            "function": {
                "arguments": "\"Cargo.toml\"}"
            }
        })],
    )
    .expect("merge tool call argument suffix");

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["id"], "call-1");
    assert_eq!(calls[0]["type"], "function");
    assert_eq!(calls[0]["function"]["name"], "read_file");
    assert_eq!(
        calls[0]["function"]["arguments"],
        "{\"path\":\"Cargo.toml\"}"
    );
}

#[test]
fn merge_streaming_tool_calls_rejects_missing_index() {
    let mut calls = Vec::new();

    let error = merge_streaming_tool_calls(
        &mut calls,
        &[json!({
            "id": "call-1",
            "type": "function",
            "function": {
                "name": "read_file",
                "arguments": "{}"
            }
        })],
    )
    .expect_err("missing index should fail");

    assert!(error.to_string().contains("tool_calls[0] missing index"));
    assert!(calls.is_empty());
}

#[test]
fn converts_history_to_codex_responses_input_items() {
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: json!("Follow the repo rules."),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"text","text":"Need a tool."},
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"Cargo.toml"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"[package]"}
            ]),
        },
    ];

    let input = to_codex_input_items(&messages);
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[0]["role"], "assistant");
    assert_eq!(input[0]["content"][0]["type"], "output_text");
    assert_eq!(input[1]["type"], "function_call");
    assert_eq!(input[1]["call_id"], "tool-1");
    assert_eq!(input[2]["type"], "function_call_output");
    assert_eq!(input[2]["call_id"], "tool-1");
    assert_eq!(input[2]["output"], "[package]");
}

#[test]
fn preserves_mixed_user_text_and_multiple_tool_results_for_codex_inputs() {
    let messages = vec![Message {
        role: "user".to_string(),
        content: json!([
            {"type":"text","text":"First result follows."},
            {"type":"tool_result","tool_use_id":"tool-1","content":"alpha"},
            {"type":"text","text":"Second result follows."},
            {"type":"tool_result","tool_use_id":"tool-2","content":"beta"}
        ]),
    }];

    let input = to_codex_input_items(&messages);
    assert_eq!(input.len(), 4);
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[0]["content"][0]["text"], "First result follows.");
    assert_eq!(input[1]["type"], "function_call_output");
    assert_eq!(input[1]["call_id"], "tool-1");
    assert_eq!(input[1]["output"], "alpha");
    assert_eq!(input[2]["type"], "message");
    assert_eq!(input[2]["content"][0]["text"], "Second result follows.");
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "tool-2");
    assert_eq!(input[3]["output"], "beta");
}

#[test]
fn parses_codex_responses_output_into_text_and_tool_use_blocks() {
    let response = parse_codex_response(&json!({
        "status": "completed",
        "usage": {
            "input_tokens": 11,
            "output_tokens": 7
        },
        "output": [
            {
                "type": "message",
                "content": [
                    {"type": "output_text", "text": "Need to inspect a file."}
                ]
            },
            {
                "type": "function_call",
                "call_id": "call-1",
                "name": "read_file",
                "arguments": "{\"path\":\"Cargo.toml\"}"
            }
        ]
    }))
    .unwrap();

    assert_eq!(response.stop_reason, Some("completed".to_string()));
    assert_eq!(response.usage.unwrap().input_tokens, 11);
    assert_eq!(response.content.len(), 2);
    match &response.content[0] {
        ContentBlock::Text { text } => {
            assert_eq!(text, "Need to inspect a file.");
        }
        other => panic!("expected text block, got {other:?}"),
    }
    match &response.content[1] {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "call-1");
            assert_eq!(name, "read_file");
            assert_eq!(*input, json!({"path":"Cargo.toml"}));
        }
        other => panic!("expected tool_use block, got {other:?}"),
    }
}

#[test]
fn codex_responses_request_includes_reasoning_effort_when_selected() {
    let request = build_codex_responses_request(
        "gpt-5.4",
        &[
            Message {
                role: "system".to_string(),
                content: json!("Follow project instructions."),
            },
            Message {
                role: "user".to_string(),
                content: json!("Hello"),
            },
        ],
        &[],
        Some("high"),
    )
    .unwrap();

    assert_eq!(request["model"], "gpt-5.4");
    assert_eq!(request["stream"], true);
    assert_eq!(request["reasoning"]["effort"], "high");
    assert_eq!(request["instructions"], "Follow project instructions.");
    assert_eq!(request["input"][0]["role"], "user");
}

#[test]
fn codex_responses_tools_use_upstream_schema_defaults_and_chatgpt_normalization() {
    let request = build_codex_responses_request(
        "gpt-5.4",
        &[Message {
            role: "user".to_string(),
            content: json!("Hello"),
        }],
        &[json!({
            "name": "team_create",
            "description": "Create a team task graph",
            "input_schema": {
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array"
                    },
                    "command": { "type": "string" },
                    "program": { "type": "string" }
                },
                "anyOf": [
                    { "required": ["command"] },
                    { "required": ["program"] }
                ]
            }
        })],
        None,
    )
    .unwrap();

    let parameters = &request["tools"][0]["parameters"];
    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["strict"], false);
    assert_eq!(parameters["type"], "object");
    assert!(parameters.get("anyOf").is_none());
    assert_eq!(parameters["properties"]["tasks"]["type"], "array");
    assert_eq!(parameters["properties"]["tasks"]["items"]["type"], "string");
}

#[test]
fn codex_responses_tools_enable_strict_for_structured_plan_schema() {
    let tool = ExitPlanModeTool;
    let request = build_codex_responses_request(
        "gpt-5.4",
        &[Message {
            role: "user".to_string(),
            content: json!("Plan the implementation."),
        }],
        &[json!({
            "name": tool.name(),
            "description": tool.description(),
            "input_schema": tool.input_schema()
        })],
        None,
    )
    .unwrap();

    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["name"], "exit_plan_mode");
    assert_eq!(request["tools"][0]["strict"], true);
    assert_eq!(
        request["tools"][0]["parameters"]["properties"]["proposed_plan"]["required"],
        json!(["summary", "steps", "validation"])
    );
}

#[test]
fn codex_responses_tools_disable_strict_when_nested_object_is_not_strict_compatible() {
    let request = build_codex_responses_request(
        "gpt-5.4",
        &[Message {
            role: "user".to_string(),
            content: json!("Plan the implementation."),
        }],
        &[json!({
            "name": "nested_tool",
            "description": "Tool with a nested object schema",
            "input_schema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "payload": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" }
                        }
                    }
                }
            }
        })],
        None,
    )
    .unwrap();

    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["strict"], false);
}

#[test]
fn codex_stream_events_collect_output_items_usage_and_text_deltas() {
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut streamed_text = String::new();
    let mut deltas = Vec::new();

    let mut on_delta = |event: LlmStreamEvent| deltas.push(event);
    let mut on_delta_option: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)> = Some(&mut on_delta);
    assert!(
        !apply_codex_stream_event(
            &json!({"type":"response.output_text.delta","delta":"Hello"}),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut on_delta_option,
        )
        .unwrap()
    );

    let mut no_delta_callback: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)> = None;
    assert!(
        !apply_codex_stream_event(
            &json!({
                "type":"response.output_item.done",
                "item":{
                    "type":"message",
                    "content":[{"type":"output_text","text":"Hello"}]
                }
            }),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut no_delta_callback,
        )
        .unwrap()
    );

    assert!(
        apply_codex_stream_event(
            &json!({
                "type":"response.completed",
                "response":{
                    "id":"resp-1",
                    "usage":{"input_tokens":11,"output_tokens":7}
                }
            }),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut no_delta_callback,
        )
        .unwrap()
    );

    assert_eq!(deltas, vec![LlmStreamEvent::TextDelta("Hello".to_string())]);
    assert_eq!(streamed_text, "Hello");
    assert_eq!(output_items.len(), 1);
    assert_eq!(output_items[0]["type"], "message");
    assert_eq!(usage.as_ref().unwrap()["input_tokens"], 11);
    assert_eq!(usage.as_ref().unwrap()["output_tokens"], 7);
}

#[test]
fn codex_stream_reasoning_delta_is_reported_without_agent_text() {
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut streamed_text = String::new();
    let mut events = Vec::new();

    let mut on_event = |event: LlmStreamEvent| events.push(event);
    let mut on_event_option: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)> = Some(&mut on_event);

    assert!(
        !apply_codex_stream_event(
            &json!({"type":"response.reasoning_summary_text.delta","delta":"checking files"}),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut on_event_option,
        )
        .unwrap()
    );

    assert_eq!(streamed_text, "");
    assert_eq!(
        events,
        vec![LlmStreamEvent::ReasoningDelta("checking files".to_string())]
    );
}

#[test]
fn codex_stream_conversation_item_done_is_treated_as_output_item() {
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut streamed_text = String::new();
    let mut no_delta_callback: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)> = None;

    assert!(
        !apply_codex_stream_event(
            &json!({
                "type":"conversation.item.done",
                "item":{
                    "type":"message",
                    "role":"assistant",
                    "content":[{"type":"output_text","text":"Hello from conversation item"}]
                }
            }),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut no_delta_callback,
        )
        .unwrap()
    );

    let response = parse_codex_response(&super::openai_compatible::build_codex_stream_response(
        output_items,
        usage,
        streamed_text,
        "completed",
    ))
    .unwrap();

    assert_eq!(response.content.len(), 1);
    match &response.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "Hello from conversation item"),
        other => panic!("expected text block, got {other:?}"),
    }
}

#[test]
fn codex_stream_output_item_added_is_upserted_by_done_item() {
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut streamed_text = String::new();
    let mut no_delta_callback: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)> = None;

    assert!(
        !apply_codex_stream_event(
            &json!({
                "type":"response.output_item.added",
                "item":{
                    "id":"msg_1",
                    "type":"message",
                    "role":"assistant",
                    "content":[{"type":"output_text","text":"partial"}]
                }
            }),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut no_delta_callback,
        )
        .unwrap()
    );

    assert!(
        !apply_codex_stream_event(
            &json!({
                "type":"response.output_item.done",
                "item":{
                    "id":"msg_1",
                    "type":"message",
                    "role":"assistant",
                    "content":[{"type":"output_text","text":"final"}]
                }
            }),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut no_delta_callback,
        )
        .unwrap()
    );

    let response = parse_codex_response(&super::openai_compatible::build_codex_stream_response(
        output_items,
        usage,
        streamed_text,
        "completed",
    ))
    .unwrap();

    assert_eq!(response.content.len(), 1);
    match &response.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "final"),
        other => panic!("expected text block, got {other:?}"),
    }
}

#[test]
fn codex_stream_output_item_matches_mixed_id_and_call_id() {
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut streamed_text = String::new();
    let mut no_delta_callback: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)> = None;

    assert!(
        !apply_codex_stream_event(
            &json!({
                "type":"response.output_item.added",
                "item":{
                    "id":"tool_1",
                    "type":"function_call",
                    "name":"bash",
                    "arguments":"{}"
                }
            }),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut no_delta_callback,
        )
        .unwrap()
    );

    assert!(
        !apply_codex_stream_event(
            &json!({
                "type":"response.output_item.done",
                "item":{
                    "call_id":"tool_1",
                    "type":"function_call",
                    "name":"bash",
                    "arguments":"{\"cmd\":\"pwd\"}"
                }
            }),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut no_delta_callback,
        )
        .unwrap()
    );

    assert_eq!(output_items.len(), 1);
    assert_eq!(output_items[0]["call_id"], "tool_1");
    assert_eq!(output_items[0]["arguments"], "{\"cmd\":\"pwd\"}");
}

#[test]
fn codex_stream_response_done_marks_completion() {
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut streamed_text = String::new();
    let mut no_delta_callback: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)> = None;

    assert!(
        apply_codex_stream_event(
            &json!({
                "type":"response.done",
                "response":{
                    "id":"resp-1",
                    "usage":{"input_tokens":5,"output_tokens":3}
                }
            }),
            &mut output_items,
            &mut usage,
            &mut streamed_text,
            &mut no_delta_callback,
        )
        .unwrap()
    );

    assert_eq!(usage.as_ref().unwrap()["input_tokens"], 5);
    assert_eq!(usage.as_ref().unwrap()["output_tokens"], 3);
}

#[test]
fn codex_stream_response_failed_preserves_structured_error_code() {
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut streamed_text = String::new();
    let mut no_delta_callback: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)> = None;

    let error = apply_codex_stream_event(
        &json!({
            "type": "response.failed",
            "response": {
                "error": {
                    "code": "context_length_exceeded",
                    "message": "Your input exceeds the context window of this model."
                }
            }
        }),
        &mut output_items,
        &mut usage,
        &mut streamed_text,
        &mut no_delta_callback,
    )
    .expect_err("response.failed should error");

    assert!(is_context_window_error(&error));
}

#[test]
fn codex_stream_delta_is_used_when_done_item_has_no_text() {
    let response = parse_codex_response(&super::openai_compatible::build_codex_stream_response(
        vec![json!({
            "type": "reasoning",
            "summary": []
        })],
        None,
        "Hello from stream".to_string(),
        "completed",
    ))
    .unwrap();

    assert_eq!(response.content.len(), 1);
    match &response.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "Hello from stream"),
        other => panic!("expected text block, got {other:?}"),
    }
}

#[test]
fn parses_tool_arguments_from_string_and_object() {
    assert_eq!(
        parse_tool_arguments(&json!("{\"path\":\"Cargo.toml\"}")).unwrap(),
        json!({"path":"Cargo.toml"})
    );
    assert_eq!(
        parse_tool_arguments(&json!({"path":"Cargo.toml"})).unwrap(),
        json!({"path":"Cargo.toml"})
    );
}

#[test]
fn extracts_text_from_openai_content_array() {
    assert_eq!(
        extract_message_text(Some(&json!([
            {"type":"text","text":"hello"},
            {"type":"text","text":"world"}
        ]))),
        Some("hello\n\nworld".to_string())
    );
}

#[test]
fn converts_tool_history_to_ollama_messages() {
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"text","text":"Need a tool."},
                {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"Cargo.toml"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"[package]"}
            ]),
        },
    ];

    let ollama_messages = to_ollama_messages(&messages);
    assert_eq!(ollama_messages[0]["role"], "assistant");
    assert_eq!(
        ollama_messages[0]["tool_calls"][0]["function"]["name"],
        "read_file"
    );
    assert_eq!(ollama_messages[1]["role"], "tool");
    assert_eq!(ollama_messages[1]["tool_name"], "read_file");
}

#[test]
fn suggests_larger_num_ctx_for_plan_and_tool_heavy_turns() {
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: json!("<proposed_plan>\n- [pending] Inspect src/\n</proposed_plan>"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type":"tool_use","id":"tool-1","name":"list_files","input":{"path":"src"}}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                {"type":"tool_result","tool_use_id":"tool-1","content":"src/main.rs\nsrc/agent.rs"}
            ]),
        },
        Message {
            role: "user".to_string(),
            content: json!("<agent_runtime>\nphase: tool_results_available\n</agent_runtime>"),
        },
    ];

    assert_eq!(suggest_ollama_num_ctx(&messages, &[], true), Some(32768));
}

#[test]
fn prefers_explicit_num_ctx_override_for_ollama_options() {
    let messages = vec![Message {
        role: "user".to_string(),
        content: json!("hello"),
    }];

    let options = build_ollama_options(&messages, &[], true, Some(65536)).unwrap();
    assert_eq!(options["num_ctx"], json!(65536));
}

#[test]
fn derives_context_budget_for_codex_like_models() {
    let budget = model_context_budget("gpt-5.1-codex").expect("budget");
    assert_eq!(budget.context_window_tokens, 200_000);
    assert!(budget.compact_threshold_tokens < budget.context_window_tokens);
}

#[test]
fn derives_context_budget_for_deepseek_v4_models() {
    let budget = model_context_budget("deepseek-v4-preview").expect("budget");
    assert_eq!(budget.context_window_tokens, 1_000_000);
    assert!(budget.compact_threshold_tokens > 900_000);
}

#[test]
fn does_not_infer_deepseek_long_context_from_unlisted_versions() {
    assert!(model_context_budget("deepseek-v3").is_none());
}

#[test]
fn applies_ollama_stream_event_deltas_and_tool_calls() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut stop_reason = None;
    let mut input_tokens = 0u32;
    let mut output_tokens = 0u32;
    let mut deltas = Vec::new();

    let done = apply_ollama_stream_event(
        &json!({"message":{"content":"Hello"}}),
        &mut text,
        &mut tool_calls,
        &mut stop_reason,
        &mut input_tokens,
        &mut output_tokens,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    assert!(!done);

    let done = apply_ollama_stream_event(
        &json!({
            "message":{
                "content":" world",
                "tool_calls":[{"function":{"name":"read_file","arguments":{"path":"Cargo.toml"}}}]
            },
            "done": true,
            "done_reason": "stop",
            "prompt_eval_count": 12,
            "eval_count": 6
        }),
        &mut text,
        &mut tool_calls,
        &mut stop_reason,
        &mut input_tokens,
        &mut output_tokens,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    assert!(done);

    assert_eq!(text, "Hello world");
    assert_eq!(
        deltas,
        vec![
            LlmStreamEvent::TextDelta("Hello".to_string()),
            LlmStreamEvent::TextDelta(" world".to_string())
        ]
    );
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].name, "read_file");
    assert_eq!(tool_calls[0].arguments, json!({"path":"Cargo.toml"}));
    assert_eq!(stop_reason, Some("stop".to_string()));
    assert_eq!(input_tokens, 12);
    assert_eq!(output_tokens, 6);
}

#[test]
fn rejects_ollama_streams_without_final_done_event() {
    let error = ensure_ollama_stream_completed(false, "http://localhost:11434/api/chat")
        .expect_err("missing done event should fail");
    assert!(
        error
            .to_string()
            .contains("ended before the final done event")
    );
}

#[test]
fn deduplicates_repeated_ollama_stream_tool_calls() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut stop_reason = None;
    let mut input_tokens = 0u32;
    let mut output_tokens = 0u32;

    apply_ollama_stream_event(
        &json!({
            "message":{
                "tool_calls":[{"function":{"name":"read_file","arguments":{"path":"Cargo.toml"}}}]
            }
        }),
        &mut text,
        &mut tool_calls,
        &mut stop_reason,
        &mut input_tokens,
        &mut output_tokens,
        &mut |_| {},
    )
    .unwrap();

    apply_ollama_stream_event(
        &json!({
            "message":{
                "tool_calls":[{"function":{"name":"read_file","arguments":{"path":"Cargo.toml"}}}]
            }
        }),
        &mut text,
        &mut tool_calls,
        &mut stop_reason,
        &mut input_tokens,
        &mut output_tokens,
        &mut |_| {},
    )
    .unwrap();

    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].name, "read_file");
    assert_eq!(tool_calls[0].arguments, json!({"path":"Cargo.toml"}));
}

#[test]
fn ignores_incomplete_ollama_stream_tool_calls_until_arguments_are_complete() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut stop_reason = None;
    let mut input_tokens = 0u32;
    let mut output_tokens = 0u32;

    apply_ollama_stream_event(
        &json!({
            "message":{
                "tool_calls":[{"function":{"name":"read_file","arguments":"{\"path\":"}}]
            }
        }),
        &mut text,
        &mut tool_calls,
        &mut stop_reason,
        &mut input_tokens,
        &mut output_tokens,
        &mut |_| {},
    )
    .unwrap();

    assert!(tool_calls.is_empty());

    apply_ollama_stream_event(
        &json!({
            "message":{
                "tool_calls":[{"function":{"name":"read_file","arguments":{"path":"Cargo.toml"}}}]
            }
        }),
        &mut text,
        &mut tool_calls,
        &mut stop_reason,
        &mut input_tokens,
        &mut output_tokens,
        &mut |_| {},
    )
    .unwrap();

    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].name, "read_file");
}

#[test]
fn bypasses_proxy_for_local_ollama_hosts() {
    assert!(should_bypass_proxy("http://localhost:11434"));
    assert!(should_bypass_proxy("http://127.0.0.1:11434"));
    assert!(!should_bypass_proxy("https://openrouter.ai/api/v1"));
}

#[test]
fn detects_openai_stream_idle_timeout_as_retryable() {
    let error = anyhow::anyhow!("OpenAI-compatible SSE stream produced no events for 120 seconds");
    assert!(is_openai_stream_idle_error(&error));

    let parse_error = anyhow::anyhow!("Failed to parse SSE payload: expected value");
    assert!(!is_openai_stream_idle_error(&parse_error));
}
