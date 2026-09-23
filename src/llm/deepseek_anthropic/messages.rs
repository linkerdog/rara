//! Converts RARA's internal [`Message`] history into Anthropic Messages
//! request shapes.

use serde_json::{Value, json};

use crate::agent::Message;
use crate::llm::shared::extract_message_text;
use crate::model_context::{MODEL_CONTEXT_BLOCK_TYPE, model_context_text};

pub(super) fn to_anthropic_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut out = Vec::new();
    for message in messages {
        if message.role == "system" {
            // System history is not always a plain string: compaction
            // boundaries and carry-over notes store it as an array of text
            // blocks (see `build_compact_boundary_message`), same as user
            // messages can.
            if let Some(text) = extract_message_text(Some(&message.content))
                && !text.trim().is_empty()
            {
                system_parts.push(text);
            }
            continue;
        }
        let role = if message.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        let content = to_anthropic_message_content(&message.content);
        let is_empty = match &content {
            Value::String(text) => text.trim().is_empty(),
            Value::Array(items) => items.is_empty(),
            _ => false,
        };
        if is_empty {
            continue;
        }
        out.push(json!({"role": role, "content": content}));
    }
    let system = (!system_parts.is_empty()).then(|| system_parts.join("\n\n"));
    (system, out)
}

/// Converts one message's content into Anthropic content blocks. Our
/// internal block shapes (`text`, `tool_use`, `tool_result`) already match
/// Anthropic's wire format directly; the one reconstruction needed is a
/// `thinking` block (with its `signature`) from the `ProviderMetadata` slot
/// this backend's own responses populate, which DeepSeek requires replayed
/// on every subsequent turn while tools are in play.
fn to_anthropic_message_content(content: &Value) -> Value {
    if let Some(text) = content.as_str() {
        return Value::String(text.to_string());
    }
    let Some(items) = content.as_array() else {
        return content.clone();
    };

    let mut thinking_block = None;
    let mut blocks = Vec::with_capacity(items.len());
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => blocks.push(item.clone()),
            Some(MODEL_CONTEXT_BLOCK_TYPE) => {
                if let Some(text) = model_context_text(item) {
                    blocks.push(json!({"type": "text", "text": text}));
                }
            }
            Some("tool_use") => blocks.push(json!({
                "type": "tool_use",
                "id": item.get("id").and_then(Value::as_str).unwrap_or_default(),
                "name": item.get("name").and_then(Value::as_str).unwrap_or_default(),
                "input": item.get("input").cloned().unwrap_or_else(|| json!({})),
            })),
            Some("tool_result") => {
                let mut block = json!({
                    "type": "tool_result",
                    "tool_use_id": item.get("tool_use_id").and_then(Value::as_str).unwrap_or_default(),
                    "content": item.get("content").and_then(Value::as_str).unwrap_or_default(),
                });
                if item.get("is_error").and_then(Value::as_bool) == Some(true) {
                    block["is_error"] = json!(true);
                }
                blocks.push(block);
            }
            Some("provider_metadata")
                if item.get("provider").and_then(Value::as_str)
                    == Some(super::THINKING_PROVIDER)
                    && item.get("key").and_then(Value::as_str) == Some(super::THINKING_KEY) =>
            {
                if let Some(value) = item.get("value") {
                    thinking_block = Some(json!({
                        "type": "thinking",
                        "thinking": value.get("thinking").and_then(Value::as_str).unwrap_or_default(),
                        "signature": value.get("signature").and_then(Value::as_str).unwrap_or_default(),
                    }));
                }
            }
            _ => {}
        }
    }
    if let Some(block) = thinking_block {
        blocks.insert(0, block);
    }
    Value::Array(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_conversion_reconstructs_leading_thinking_block_from_provider_metadata() {
        let messages = [Message {
            role: "assistant".to_string(),
            content: json!([
                {"type": "provider_metadata", "provider": "deepseek", "key": "thinking",
                 "value": {"thinking": "reasoning", "signature": "sig-1"}},
                {"type": "text", "text": "answer"},
                {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "Paris"}},
            ]),
        }];

        let (system, out) = to_anthropic_messages(&messages);
        assert_eq!(system, None);
        assert_eq!(
            out[0]["content"],
            json!([
                {"type": "thinking", "thinking": "reasoning", "signature": "sig-1"},
                {"type": "text", "text": "answer"},
                {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "Paris"}},
            ])
        );
    }

    #[test]
    fn message_conversion_folds_tool_results_into_user_role() {
        let messages = [Message {
            role: "user".to_string(),
            content: json!([
                {"type": "tool_result", "tool_use_id": "call_1", "content": "18C, cloudy"},
            ]),
        }];

        let (_, out) = to_anthropic_messages(&messages);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(
            out[0]["content"],
            json!([{"type": "tool_result", "tool_use_id": "call_1", "content": "18C, cloudy"}])
        );
    }

    #[test]
    fn message_conversion_collects_system_text() {
        let messages = [Message {
            role: "system".to_string(),
            content: json!("be helpful"),
        }];
        let (system, out) = to_anthropic_messages(&messages);
        assert_eq!(system, Some("be helpful".to_string()));
        assert!(out.is_empty());
    }

    #[test]
    fn message_conversion_renders_array_shaped_system_content_from_compaction() {
        let messages = [Message {
            role: "system".to_string(),
            content: json!([{"type": "text", "text": "COMPACTION BOUNDARY: carried-over summary"}]),
        }];
        let (system, _) = to_anthropic_messages(&messages);
        assert_eq!(
            system,
            Some("COMPACTION BOUNDARY: carried-over summary".to_string())
        );
    }
}
