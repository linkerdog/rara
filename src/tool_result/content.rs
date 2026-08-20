use serde_json::Value;

use crate::agent::Message;

pub(crate) struct ToolResultContentCandidate {
    pub(crate) message_index: usize,
    pub(crate) block_index: usize,
    pub(crate) chars: usize,
}

pub(super) fn tool_result_content_candidates(
    messages: &[Message],
) -> Vec<ToolResultContentCandidate> {
    let mut candidates = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let Some(blocks) = message.content.as_array() else {
            continue;
        };
        for (block_index, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(content) = block.get("content").and_then(Value::as_str) else {
                continue;
            };
            candidates.push(ToolResultContentCandidate {
                message_index,
                block_index,
                chars: content.chars().count(),
            });
        }
    }
    candidates
}

pub(super) fn tool_result_content_mut(
    messages: &mut [Message],
    message_index: usize,
    block_index: usize,
) -> Option<&mut String> {
    messages
        .get_mut(message_index)?
        .content
        .as_array_mut()?
        .get_mut(block_index)?
        .get_mut("content")
        .and_then(|value| match value {
            Value::String(content) => Some(content),
            _ => None,
        })
}
