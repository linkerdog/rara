//! Assembles one streamed Anthropic `content_block_*` sequence into RARA's
//! internal [`ContentBlock`] vocabulary, plus the usage-payload helpers used
//! while doing so.

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::llm::{ContentBlock, TokenUsage};

/// Accumulates one streamed Anthropic `content_block_*` sequence into our
/// internal [`ContentBlock`] vocabulary, in the order blocks close — which is
/// also their natural generation order, so no reordering is needed the way
/// `chat/completions` needs it for DeepSeek's inline DSML fallback.
#[derive(Default)]
pub(super) struct AnthropicBlockAssembler {
    in_progress: std::collections::HashMap<u64, PendingBlock>,
    finished: Vec<ContentBlock>,
}

enum PendingBlock {
    Text(String),
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        partial_json: String,
    },
}

impl AnthropicBlockAssembler {
    /// Seeds a pending block from its `content_block_start` payload and
    /// returns any initial non-empty text/thinking it already carries, so
    /// the caller can forward it as a delta. Anthropic's own protocol
    /// always starts these blocks empty and streams content only through
    /// later `content_block_delta` events, but nothing in the wire format
    /// guarantees that, so this seeds from whatever the start payload
    /// actually carries instead of assuming it is always empty.
    ///
    /// An unrecognized or missing `content_block.type` is a protocol
    /// surprise this backend doesn't know how to interpret (e.g. Anthropic's
    /// own `redacted_thinking`, which — like `thinking` — may need its
    /// content replayed on a later turn); silently dropping it could lose
    /// data the next request needs, so it errors instead.
    pub(super) fn start(&mut self, payload: &Value) -> Result<(Option<String>, Option<String>)> {
        let Some(index) = payload.get("index").and_then(Value::as_u64) else {
            return Ok((None, None));
        };
        let block = payload.get("content_block");
        let mut initial_text = None;
        let mut initial_thinking = None;
        let pending = match block.and_then(|b| b.get("type")).and_then(Value::as_str) {
            Some("text") => {
                let text = block
                    .and_then(|b| b.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !text.is_empty() {
                    initial_text = Some(text.to_string());
                }
                PendingBlock::Text(text.to_string())
            }
            Some("thinking") => {
                let thinking = block
                    .and_then(|b| b.get("thinking"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let signature = block
                    .and_then(|b| b.get("signature"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !thinking.is_empty() {
                    initial_thinking = Some(thinking.to_string());
                }
                PendingBlock::Thinking {
                    thinking: thinking.to_string(),
                    signature: signature.to_string(),
                }
            }
            Some("tool_use") => {
                let id = block
                    .and_then(|b| b.get("id"))
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        anyhow!(
                            "DeepSeek Anthropic-compatible stream content_block_start[{index}] tool_use missing id"
                        )
                    })?
                    .to_string();
                let name = block
                    .and_then(|b| b.get("name"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| {
                        anyhow!(
                            "DeepSeek Anthropic-compatible stream content_block_start[{index}] tool_use missing name"
                        )
                    })?
                    .to_string();
                let partial_json = block
                    .and_then(|b| b.get("input"))
                    .filter(|input| input.as_object().is_some_and(|object| !object.is_empty()))
                    .map(ToString::to_string)
                    .unwrap_or_default();
                PendingBlock::ToolUse {
                    id,
                    name,
                    partial_json,
                }
            }
            Some(other) => {
                return Err(anyhow!(
                    "DeepSeek Anthropic-compatible stream content_block_start[{index}] has an unsupported content_block type \"{other}\""
                ));
            }
            None => {
                return Err(anyhow!(
                    "DeepSeek Anthropic-compatible stream content_block_start[{index}] is missing content_block.type"
                ));
            }
        };
        self.in_progress.insert(index, pending);
        Ok((initial_text, initial_thinking))
    }

    pub(super) fn delta_text(&mut self, payload: &Value) -> Option<String> {
        let index = payload.get("index").and_then(Value::as_u64)?;
        let delta = payload.get("delta")?;
        if delta.get("type").and_then(Value::as_str) != Some("text_delta") {
            return None;
        }
        let text = delta.get("text").and_then(Value::as_str)?.to_string();
        if let Some(PendingBlock::Text(buffer)) = self.in_progress.get_mut(&index) {
            buffer.push_str(&text);
        }
        Some(text)
    }

    pub(super) fn delta_thinking(&mut self, payload: &Value) -> Option<String> {
        let index = payload.get("index").and_then(Value::as_u64)?;
        let delta = payload.get("delta")?;
        match delta.get("type").and_then(Value::as_str) {
            Some("thinking_delta") => {
                let text = delta.get("thinking").and_then(Value::as_str)?.to_string();
                if let Some(PendingBlock::Thinking { thinking, .. }) =
                    self.in_progress.get_mut(&index)
                {
                    thinking.push_str(&text);
                }
                Some(text)
            }
            Some("signature_delta") => {
                let signature = delta.get("signature").and_then(Value::as_str)?;
                if let Some(PendingBlock::Thinking { signature: sig, .. }) =
                    self.in_progress.get_mut(&index)
                {
                    sig.push_str(signature);
                }
                None
            }
            Some("input_json_delta") => {
                let partial = delta.get("partial_json").and_then(Value::as_str)?;
                if let Some(PendingBlock::ToolUse { partial_json, .. }) =
                    self.in_progress.get_mut(&index)
                {
                    partial_json.push_str(partial);
                }
                None
            }
            _ => None,
        }
    }

    pub(super) fn stop(&mut self, payload: &Value) -> Result<()> {
        let Some(index) = payload.get("index").and_then(Value::as_u64) else {
            return Ok(());
        };
        let Some(pending) = self.in_progress.remove(&index) else {
            return Ok(());
        };
        let block = match pending {
            PendingBlock::Text(text) => (!text.is_empty()).then(|| ContentBlock::Text { text }),
            PendingBlock::Thinking {
                thinking,
                signature,
            } => Some(ContentBlock::ProviderMetadata {
                provider: super::THINKING_PROVIDER.to_string(),
                key: super::THINKING_KEY.to_string(),
                value: json!({"thinking": thinking, "signature": signature}),
            }),
            PendingBlock::ToolUse {
                id,
                name,
                partial_json,
            } => Some(ContentBlock::ToolUse {
                id,
                name,
                input: parse_tool_use_input(&partial_json)?,
            }),
        };
        if let Some(block) = block {
            self.finished.push(block);
        }
        Ok(())
    }

    pub(super) fn into_content(self) -> Vec<ContentBlock> {
        self.finished
    }
}

/// Unlike `chat/completions`' `parse_tool_arguments` (which this mirrors),
/// truncated or malformed accumulated JSON is propagated as an error rather
/// than silently substituted with `{}` — an empty-argument tool call is not
/// a safe stand-in for a decoding failure.
fn parse_tool_use_input(partial_json: &str) -> Result<Value> {
    let trimmed = partial_json.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(trimmed).map_err(|error| {
        anyhow!(
            "DeepSeek Anthropic-compatible stream tool_use arguments are not valid JSON: {error}"
        )
    })
}

/// Merges an Anthropic-style usage payload onto a possibly-partial prior
/// one. DeepSeek's `message_delta.usage` has been observed carrying a full
/// snapshot (superseding `message_start`'s), but Anthropic's own documented
/// behavior only guarantees `output_tokens` there; merging field-by-field
/// is correct either way instead of assuming which fields a given event
/// actually repeats.
pub(super) fn merge_usage(base: Option<Value>, update: &Value) -> Value {
    match (base, update) {
        (Some(Value::Object(mut existing)), Value::Object(new_fields)) => {
            existing.extend(new_fields.clone());
            Value::Object(existing)
        }
        _ => update.clone(),
    }
}

pub(super) fn parse_anthropic_token_usage(usage: &Value) -> TokenUsage {
    let field = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0) as u32;
    TokenUsage {
        input_tokens: field("input_tokens"),
        output_tokens: field("output_tokens"),
        cache_hit_tokens: field("cache_read_input_tokens"),
        cache_miss_tokens: field("cache_creation_input_tokens"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_assembler_separates_thinking_text_and_tool_use_by_construction() -> Result<()> {
        let mut assembler = AnthropicBlockAssembler::default();
        assembler.start(&json!({"index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}))?;
        assert_eq!(
            assembler.delta_thinking(
                &json!({"index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}})
            ),
            Some("hmm".to_string())
        );
        assembler.delta_thinking(
            &json!({"index": 0, "delta": {"type": "signature_delta", "signature": "sig-1"}}),
        );
        assembler.stop(&json!({"index": 0}))?;

        assembler.start(&json!({"index": 1, "content_block": {"type": "text", "text": ""}}))?;
        assert_eq!(
            assembler.delta_text(
                &json!({"index": 1, "delta": {"type": "text_delta", "text": "Let me check.\n"}})
            ),
            Some("Let me check.\n".to_string())
        );
        assembler.stop(&json!({"index": 1}))?;

        assembler.start(&json!({"index": 2, "content_block": {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {}}}))?;
        for chunk in ["{", "\"city\"", ":", "\"Paris\"", "}"] {
            assembler.delta_thinking(
                &json!({"index": 2, "delta": {"type": "input_json_delta", "partial_json": chunk}}),
            );
        }
        assembler.stop(&json!({"index": 2}))?;

        let content = assembler.into_content();
        assert!(matches!(
            &content[0],
            ContentBlock::ProviderMetadata { provider, key, value }
                if provider == "deepseek" && key == "thinking"
                    && value["thinking"] == "hmm" && value["signature"] == "sig-1"
        ));
        assert!(matches!(&content[1], ContentBlock::Text { text } if text == "Let me check.\n"));
        assert!(matches!(
            &content[2],
            ContentBlock::ToolUse { id, name, input }
                if id == "call_1" && name == "get_weather" && input["city"] == "Paris"
        ));
        Ok(())
    }

    #[test]
    fn block_assembler_seeds_and_emits_non_empty_initial_content_block_start() -> Result<()> {
        let mut assembler = AnthropicBlockAssembler::default();
        let (initial_text, initial_thinking) = assembler.start(
            &json!({"index": 0, "content_block": {"type": "thinking", "thinking": "already here", "signature": "sig-0"}}),
        )?;
        assert_eq!(initial_thinking, Some("already here".to_string()));
        assert_eq!(initial_text, None);
        assembler.stop(&json!({"index": 0}))?;

        let (initial_text, _) = assembler.start(
            &json!({"index": 1, "content_block": {"type": "text", "text": "partial answer"}}),
        )?;
        assert_eq!(initial_text, Some("partial answer".to_string()));
        assembler.stop(&json!({"index": 1}))?;

        assembler.start(&json!({
            "index": 2,
            "content_block": {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "Paris"}}
        }))?;
        assembler.stop(&json!({"index": 2}))?;

        let content = assembler.into_content();
        assert!(matches!(
            &content[0],
            ContentBlock::ProviderMetadata { key, value, .. }
                if key == "thinking" && value["thinking"] == "already here"
        ));
        assert!(matches!(&content[1], ContentBlock::Text { text } if text == "partial answer"));
        assert!(matches!(
            &content[2],
            ContentBlock::ToolUse { input, .. } if input["city"] == "Paris"
        ));
        Ok(())
    }

    #[test]
    fn block_assembler_rejects_tool_use_missing_id_or_name() {
        let mut assembler = AnthropicBlockAssembler::default();
        assert!(
            assembler
                .start(&json!({"index": 0, "content_block": {"type": "tool_use", "id": "", "name": "get_weather"}}))
                .is_err()
        );
        assert!(
            assembler
                .start(&json!({"index": 0, "content_block": {"type": "tool_use", "id": "call_1"}}))
                .is_err()
        );
    }

    #[test]
    fn block_assembler_propagates_malformed_tool_use_json_instead_of_defaulting() -> Result<()> {
        let mut assembler = AnthropicBlockAssembler::default();
        assembler.start(&json!({"index": 0, "content_block": {"type": "tool_use", "id": "call_1", "name": "get_weather"}}))?;
        assembler.delta_thinking(
            &json!({"index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"city\": \"Par"}}),
        );
        assert!(assembler.stop(&json!({"index": 0})).is_err());
        Ok(())
    }

    #[test]
    fn block_assembler_rejects_unsupported_content_block_type() {
        let mut assembler = AnthropicBlockAssembler::default();
        assert!(
            assembler
                .start(&json!({"index": 0, "content_block": {"type": "redacted_thinking", "data": "opaque"}}))
                .is_err()
        );
    }

    #[test]
    fn usage_merges_message_start_input_with_message_delta_output() {
        let after_start = merge_usage(
            None,
            &json!({"input_tokens": 301, "cache_read_input_tokens": 0, "output_tokens": 0}),
        );
        let merged = merge_usage(
            Some(after_start),
            &json!({"output_tokens": 49, "cache_read_input_tokens": 128}),
        );
        assert_eq!(merged["input_tokens"], 301);
        assert_eq!(merged["output_tokens"], 49);
        assert_eq!(merged["cache_read_input_tokens"], 128);
    }

    #[test]
    fn parses_anthropic_shaped_usage() {
        let usage = parse_anthropic_token_usage(&json!({
            "input_tokens": 200, "output_tokens": 55,
            "cache_read_input_tokens": 128, "cache_creation_input_tokens": 0
        }));
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 55);
        assert_eq!(usage.cache_hit_tokens, 128);
        assert_eq!(usage.cache_miss_tokens, 0);
    }
}
