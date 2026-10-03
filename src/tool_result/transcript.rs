use serde_json::Value;

use super::pairing::{
    has_tool_result_block, keep_or_drop_tool_results, synthetic_tool_result_blocks,
    tool_use_ids_in_blocks,
};
use crate::agent::Message;

pub fn repair_tool_result_history(history: &[Message]) -> Vec<Message> {
    let mut repaired = Vec::with_capacity(history.len());
    let mut pending_tool_uses: Vec<String> = Vec::new();

    for message in history {
        let borrowed_blocks = message.content.as_array().map(Vec::as_slice).unwrap_or(&[]);

        if message.role == "assistant" {
            flush_pending(&mut repaired, &mut pending_tool_uses);
            pending_tool_uses.extend(tool_use_ids_in_blocks(borrowed_blocks));
            repaired.push(message.clone());
            continue;
        }

        if message.role == "user" && has_tool_result_block(borrowed_blocks) {
            let kept_blocks =
                keep_or_drop_tool_results(borrowed_blocks.to_vec(), &mut pending_tool_uses);
            if !kept_blocks.is_empty() {
                repaired.push(Message {
                    role: message.role.clone(),
                    content: Value::Array(kept_blocks),
                });
            }
            continue;
        }

        flush_pending(&mut repaired, &mut pending_tool_uses);
        repaired.push(message.clone());
    }

    flush_pending(&mut repaired, &mut pending_tool_uses);
    repaired
}

fn flush_pending(repaired: &mut Vec<Message>, pending_tool_uses: &mut Vec<String>) {
    if pending_tool_uses.is_empty() {
        return;
    }
    repaired.push(Message {
        role: "user".to_string(),
        content: Value::Array(synthetic_tool_result_blocks(pending_tool_uses)),
    });
}
