//! Converts RARA's internal [`Message`] history into Anthropic Messages
//! request shapes.

use serde_json::{Value, json};

use crate::agent::Message;
use crate::llm::shared::extract_message_text;
use crate::model_context::{MODEL_CONTEXT_BLOCK_TYPE, model_context_text};

pub(super) fn to_anthropic_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    // Ids from the most recent assistant turn's `tool_use` blocks not yet
    // matched by a `tool_result`. A tool call can go permanently unanswered
    // — an approval- or plan-exit-interrupted turn abandons the rest of its
    // batch (see `execute_tool_calls` in `src/agent/execution.rs`), and the
    // repair pass that normally patches this (`repair_tool_result_history`)
    // only runs at the very start of a fresh user query, not on the
    // approval-resume path. `chat/completions` has its own
    // `flush_missing_tool_results` guarding exactly this; this mirrors it
    // for Anthropic's stricter "tool_result in the very next message" rule.
    let mut pending_tool_use_ids: Vec<String> = Vec::new();

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
        if role == "assistant" {
            flush_missing_tool_results(&mut out, &mut pending_tool_use_ids);
        }
        let blocks = as_content_blocks(to_anthropic_message_content(&message.content));
        if role == "assistant" {
            pending_tool_use_ids.extend(tool_use_ids(&blocks));
        } else {
            resolve_tool_use_ids(&blocks, &mut pending_tool_use_ids);
        }
        // Anthropic requires every `tool_use` in a turn to have its
        // `tool_result` in the very next message. The agent loop records one
        // turn's parallel tool results as several consecutive same-role
        // `Message`s (one `tool_result` block each — see
        // `execute_tool_calls`/`tool_result_message`), followed by a runtime
        // continuation nudge, also `user`-role. `chat/completions` tolerates
        // that shape (it correlates tool results by id, not position), but
        // Anthropic does not, so runs of the same role must be coalesced
        // into one message here or a multi-tool-call turn gets rejected
        // with "tool_use ids were found without tool_result blocks
        // immediately after".
        push_or_merge(&mut out, role, blocks);
    }
    flush_missing_tool_results(&mut out, &mut pending_tool_use_ids);

    let system = (!system_parts.is_empty()).then(|| system_parts.join("\n\n"));
    (system, out)
}

/// Normalizes converted message content into a flat block list so runs of
/// the same role can be merged by extending one array, regardless of
/// whether the original content was a plain string or already an array.
fn as_content_blocks(content: Value) -> Vec<Value> {
    match content {
        Value::String(text) => {
            if text.trim().is_empty() {
                Vec::new()
            } else {
                vec![json!({"type": "text", "text": text})]
            }
        }
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

fn push_or_merge(out: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last["role"] == role
        && let Some(existing) = last["content"].as_array_mut()
    {
        existing.extend(blocks);
        return;
    }
    out.push(json!({"role": role, "content": Value::Array(blocks)}));
}

fn tool_use_ids(blocks: &[Value]) -> Vec<String> {
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn resolve_tool_use_ids(blocks: &[Value], pending: &mut Vec<String>) {
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        if let Some(id) = block.get("tool_use_id").and_then(Value::as_str)
            && let Some(pos) = pending.iter().position(|pending_id| pending_id == id)
        {
            pending.remove(pos);
        }
    }
}

/// Synthesizes an error `tool_result` for every `tool_use` id that never got
/// one, mirroring `chat/completions`' `flush_missing_tool_results`.
fn flush_missing_tool_results(out: &mut Vec<Value>, pending_tool_use_ids: &mut Vec<String>) {
    if pending_tool_use_ids.is_empty() {
        return;
    }
    let blocks = pending_tool_use_ids
        .drain(..)
        .map(|id| {
            json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": "Tool execution was interrupted before a result was recorded.",
                "is_error": true,
            })
        })
        .collect::<Vec<_>>();
    push_or_merge(out, "user", blocks);
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
    fn message_conversion_merges_parallel_tool_results_into_one_message_for_anthropic_adjacency() {
        // Reproduces a real production 400: DeepSeek's Anthropic-compatible
        // endpoint rejects a request where a multi-tool_use assistant turn's
        // results are split across several consecutive `user` messages
        // (as `execute_tool_calls`/`tool_result_message` record them
        // internally, one message per tool call) instead of gathered into
        // the single message immediately following that turn.
        let messages = [
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type": "tool_use", "id": "call_1", "name": "read_file", "input": {"path": "a"}},
                    {"type": "tool_use", "id": "call_2", "name": "read_file", "input": {"path": "b"}},
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{"type": "tool_result", "tool_use_id": "call_1", "content": "A"}]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{"type": "tool_result", "tool_use_id": "call_2", "content": "B"}]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{"type": "text", "text": "continuation nudge"}]),
            },
        ];

        let (_, out) = to_anthropic_messages(&messages);
        assert_eq!(
            out.len(),
            2,
            "the tool_use turn's three trailing user messages must merge into one"
        );
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[1]["role"], "user");
        assert_eq!(
            out[1]["content"],
            json!([
                {"type": "tool_result", "tool_use_id": "call_1", "content": "A"},
                {"type": "tool_result", "tool_use_id": "call_2", "content": "B"},
                {"type": "text", "text": "continuation nudge"},
            ])
        );
    }

    #[test]
    fn message_conversion_synthesizes_missing_tool_result_before_next_assistant_turn() {
        // A second production scenario, distinct from the parallel-results
        // one above: an approval- or plan-exit-interrupted turn can abandon
        // the rest of its tool_use batch, leaving some ids with no
        // tool_result at all anywhere in history (see `execute_tool_calls`
        // in `src/agent/execution.rs`, and `repair_tool_result_history`'s
        // gap on the approval-resume path). Without a repair here, the
        // still-pending `tool_use` id from the earlier turn would ride into
        // the next assistant message unresolved, the same class of 400 this
        // module exists to avoid.
        let messages = [
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type": "tool_use", "id": "call_1", "name": "read_file", "input": {"path": "a"}},
                    {"type": "tool_use", "id": "call_2", "name": "exit_plan_mode", "input": {}},
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{"type": "tool_result", "tool_use_id": "call_1", "content": "A"}]),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([{"type": "text", "text": "next turn"}]),
            },
        ];

        let (_, out) = to_anthropic_messages(&messages);
        assert_eq!(out.len(), 3);
        assert_eq!(
            out[1]["content"],
            json!([
                {"type": "tool_result", "tool_use_id": "call_1", "content": "A"},
                {
                    "type": "tool_result",
                    "tool_use_id": "call_2",
                    "content": "Tool execution was interrupted before a result was recorded.",
                    "is_error": true
                },
            ])
        );
        assert_eq!(out[2]["role"], "assistant");
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
