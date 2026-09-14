use serde_json::json;

use super::backend::SummaryPrefix;
use super::contracts::LlmExecutionMode;
use super::types::{LlmResponse, Message};

#[test]
fn response_preserves_root_wire_shape() -> serde_json::Result<()> {
    let wire = json!({
        "content": [{"type": "text", "text": "done"}],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 120,
            "output_tokens": 8,
            "cache_hit_tokens": 100,
            "cache_miss_tokens": 20
        }
    });
    let response: LlmResponse = serde_json::from_value(wire.clone())?;
    assert_eq!(serde_json::to_value(response)?, wire);
    Ok(())
}

#[test]
fn response_preserves_unknown_usage_and_defaults_only_cache_counters() -> serde_json::Result<()> {
    let response: LlmResponse = serde_json::from_value(json!({"content": []}))?;
    assert!(response.usage.is_none());
    let response: LlmResponse = serde_json::from_value(json!({
        "content": [], "usage": {"input_tokens": 12, "output_tokens": 3}
    }))?;
    assert_eq!(
        serde_json::to_value(response.usage)?,
        json!({"input_tokens": 12, "output_tokens": 3, "cache_hit_tokens": 0, "cache_miss_tokens": 0})
    );
    Ok(())
}

#[test]
fn summary_preserves_system_messages_in_history_and_requires_the_complete_prefix()
-> anyhow::Result<()> {
    let messages = vec![
        Message {
            role: "system".into(),
            content: json!("generated instructions"),
        },
        Message {
            role: "system".into(),
            content: json!("previous compact summary"),
        },
        Message {
            role: "user".into(),
            content: json!("continue the task"),
        },
    ];
    let prefix = SummaryPrefix {
        messages: messages.clone(),
        tools: vec![],
        execution_mode: LlmExecutionMode::Execute,
    };
    let request = prefix.messages_for_summary(&messages[1..], "summarize")?;
    assert_eq!(&request[..messages.len()], &messages);
    assert_eq!(request.len(), messages.len() + 1);
    assert!(
        prefix
            .messages_for_summary(&messages[1..2], "summarize")
            .is_err()
    );
    assert!(
        prefix
            .messages_for_summary(&messages[2..], "summarize")
            .is_err()
    );
    Ok(())
}
