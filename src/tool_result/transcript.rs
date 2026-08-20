use serde_json::{Value, json};

use crate::agent::Message;

pub fn repair_tool_result_history(history: &[Message]) -> Vec<Message> {
    let mut repaired = Vec::with_capacity(history.len());
    let mut pending_tool_uses: Vec<String> = Vec::new();

    for message in history {
        if message.role == "assistant" {
            if !pending_tool_uses.is_empty() {
                repaired.push(synthetic_tool_result_message(&pending_tool_uses));
                pending_tool_uses.clear();
            }
            pending_tool_uses.extend(extract_tool_use_ids(&message.content));
            repaired.push(message.clone());
            continue;
        }

        if message.role == "user" && has_tool_result_blocks(&message.content) {
            let mut kept_blocks = Vec::new();
            if let Some(items) = message.content.as_array() {
                for item in items {
                    if item.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let Some(tool_use_id) = item.get("tool_use_id").and_then(Value::as_str)
                        else {
                            continue;
                        };
                        if let Some(pos) = pending_tool_uses.iter().position(|id| id == tool_use_id)
                        {
                            pending_tool_uses.remove(pos);
                            kept_blocks.push(item.clone());
                        }
                    } else {
                        kept_blocks.push(item.clone());
                    }
                }
            }
            if !kept_blocks.is_empty() {
                repaired.push(Message {
                    role: message.role.clone(),
                    content: Value::Array(kept_blocks),
                });
            }
            continue;
        }

        if !pending_tool_uses.is_empty() {
            repaired.push(synthetic_tool_result_message(&pending_tool_uses));
            pending_tool_uses.clear();
        }
        repaired.push(message.clone());
    }

    if !pending_tool_uses.is_empty() {
        repaired.push(synthetic_tool_result_message(&pending_tool_uses));
    }

    repaired
}

fn synthetic_tool_result_message(ids: &[String]) -> Message {
    Message {
        role: "user".to_string(),
        content: Value::Array(
            ids.iter()
                .map(|id| {
                    json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": "Tool execution was interrupted before a result was recorded.",
                        "is_error": true
                    })
                })
                .collect(),
        ),
    }
}

fn extract_tool_use_ids(content: &Value) -> Vec<String> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn has_tool_result_blocks(content: &Value) -> bool {
    content.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
    })
}
