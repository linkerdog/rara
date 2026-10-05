use serde_json::{Value, json};

/// Extracts every `tool_use` id from a content-block array.
pub fn tool_use_ids_in_blocks(blocks: &[Value]) -> Vec<String> {
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Whether any block in the array is a `tool_result`.
pub fn has_tool_result_block(blocks: &[Value]) -> bool {
    blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
}

/// Keeps every non-`tool_result` block unconditionally. A `tool_result`
/// block is kept only if its `tool_use_id` is currently `pending` (removing
/// it from `pending`); otherwise it is dropped — it has no `tool_use`
/// immediately before it (already resolved, already flushed as a synthetic
/// filler, or never existed), and passing it through gets rejected by
/// stricter wire protocols the same way an unresolved `tool_use` does.
pub fn keep_or_drop_tool_results(blocks: Vec<Value>, pending: &mut Vec<String>) -> Vec<Value> {
    blocks
        .into_iter()
        .filter(|block| {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                return true;
            }
            let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                return false;
            };
            match pending.iter().position(|pending_id| pending_id == id) {
                Some(pos) => {
                    pending.remove(pos);
                    true
                }
                None => false,
            }
        })
        .collect()
}

/// Builds one synthetic, `is_error: true` `tool_result` block per pending
/// id, draining `pending`. Used when a tool call goes permanently
/// unanswered after an interrupted batch. Apply only when resuming a new
/// request, never while a pending approval can still produce the real result.
pub fn synthetic_tool_result_blocks(pending: &mut Vec<String>) -> Vec<Value> {
    pending
        .drain(..)
        .map(|id| {
            json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": "Tool execution was interrupted before a result was recorded.",
                "is_error": true,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_matching_and_drops_unmatched_tool_results() {
        let mut pending = vec!["call_1".to_string()];
        let blocks = vec![
            json!({"type": "tool_result", "tool_use_id": "call_1", "content": "A"}),
            json!({"type": "tool_result", "tool_use_id": "call_stale", "content": "B"}),
            json!({"type": "text", "text": "keep me"}),
        ];

        let kept = keep_or_drop_tool_results(blocks, &mut pending);

        assert_eq!(
            kept,
            vec![
                json!({"type": "tool_result", "tool_use_id": "call_1", "content": "A"}),
                json!({"type": "text", "text": "keep me"}),
            ]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn synthesizes_one_error_block_per_pending_id_and_drains_pending() {
        let mut pending = vec!["call_1".to_string(), "call_2".to_string()];
        let blocks = synthetic_tool_result_blocks(&mut pending);

        assert_eq!(blocks.len(), 2);
        for block in &blocks {
            assert_eq!(block["type"], "tool_result");
            assert_eq!(block["is_error"], true);
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn extracts_tool_use_ids_and_detects_tool_result_presence() {
        let blocks = vec![
            json!({"type": "tool_use", "id": "call_1", "name": "read_file", "input": {}}),
            json!({"type": "text", "text": "hi"}),
        ];
        assert_eq!(tool_use_ids_in_blocks(&blocks), vec!["call_1".to_string()]);
        assert!(!has_tool_result_block(&blocks));

        let with_result = vec![json!({"type": "tool_result", "tool_use_id": "call_1"})];
        assert!(has_tool_result_block(&with_result));
    }
}
