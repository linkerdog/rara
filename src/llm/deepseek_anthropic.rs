//! DeepSeek's Anthropic Messages–compatible endpoint, used for `deepseek-flash`.
//!
//! `deepseek-flash` traffic through `chat/completions` occasionally leaks
//! internal DSML tool-call markup into the `content` text stream (see
//! [`super::openai_compatible::DeepseekTextStreamScrubber`]). DeepSeek's own
//! reference harness (deepseek-ai/deepseek-harness) instead serves this model
//! through an Anthropic Messages-compatible surface
//! (`https://api.deepseek.com/anthropic`), confirmed live against the real
//! API: tool-call arguments, thinking, and text each arrive as their own
//! typed `content_block_*` events, so nothing needs client-side scrubbing.
//!
//! This backend targets that surface for `deepseek-flash` specifically and
//! delegates every other [`LlmBackend`] method to a wrapped
//! [`OpenAiCompatibleBackend`] on `chat/completions`, which the caller keeps
//! configured for non-streaming and auxiliary calls (summarize, classify,
//! context budgeting, cache accounting).
//!
//! Verified live against `api.deepseek.com` (2026-09-22): a `tool_use`
//! round trip requires the assistant's `thinking` block (with its
//! `signature`) to be replayed on the next turn whenever tools are in play,
//! exactly like `reasoning_content` on the chat/completions endpoint —
//! [`to_anthropic_message_content`] reconstructs it from the
//! `ContentBlock::ProviderMetadata` slot this backend writes on responses.

use std::num::NonZeroU32;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};

use super::inference_transport::{finish_attempt, record_final_usage, record_usage, send_json};
use super::openai_compatible::OpenAiCompatibleBackend;
use super::shared::{
    ContextBudget, LlmBackend, LlmStreamEvent, LlmTurnMetadata, ProviderCacheProfile,
    http_client_for_target, next_stream_item_with_idle_timeout,
};
use crate::agent::Message;
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};
use crate::model_context::{MODEL_CONTEXT_BLOCK_TYPE, model_context_text};
use crate::model_observation::ModelRequestFingerprint;

/// The only model DeepSeek's own reference harness routes through its
/// Anthropic-compatible surface by default; other DeepSeek models stay on
/// `chat/completions` until they are verified against this endpoint too.
pub const DEEPSEEK_ANTHROPIC_MODEL: &str = "deepseek-flash";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Matches the DeepSeek reference harness's own `maxTokens` default; the
/// Messages API requires an explicit cap, unlike `chat/completions`.
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 256_000;
const THINKING_PROVIDER: &str = "deepseek";
const THINKING_KEY: &str = "thinking";

/// Only activate the Anthropic-compatible path against DeepSeek's own host:
/// the `/anthropic` mount is a DeepSeek-specific convention, not something a
/// third-party gateway serving the same `chat/completions` surface can be
/// assumed to also expose.
fn deepseek_anthropic_base_url(configured_base_url: &str) -> Option<String> {
    let host = url::Url::parse(configured_base_url)
        .ok()?
        .host_str()?
        .to_ascii_lowercase();
    (host == "api.deepseek.com").then(|| "https://api.deepseek.com/anthropic".to_string())
}

/// Wraps `fallback` in a [`DeepseekAnthropicBackend`] when `model`/`kind`
/// qualify for DeepSeek's Anthropic-compatible surface and `base_url` is
/// DeepSeek's own host; otherwise returns `fallback` unchanged.
///
/// Never fails: the one fallible step (building the HTTP client) runs
/// before `fallback` is moved into the wrapping struct, so a failure there
/// falls back to the plain backend instead of losing it.
pub(crate) fn wrap_if_eligible(
    fallback: OpenAiCompatibleBackend,
    api_key: Option<SecretString>,
    thinking: Option<bool>,
    kind: crate::config::OpenAiEndpointKind,
    base_url: &str,
    model: &str,
) -> Box<dyn LlmBackend> {
    if kind != crate::config::OpenAiEndpointKind::Deepseek || model != DEEPSEEK_ANTHROPIC_MODEL {
        return Box::new(fallback);
    }
    let Some(anthropic_base_url) = deepseek_anthropic_base_url(base_url) else {
        return Box::new(fallback);
    };
    let client = match http_client_for_target(&anthropic_base_url) {
        Ok(client) => client,
        Err(error) => {
            log::warn!(
                "Failed to build an HTTP client for DeepSeek's Anthropic-compatible endpoint, staying on chat/completions: {error}"
            );
            return Box::new(fallback);
        }
    };
    Box::new(DeepseekAnthropicBackend {
        client,
        api_key,
        base_url: anthropic_base_url,
        model: model.to_string(),
        max_output_tokens: None,
        thinking,
        fallback,
    })
}

pub(crate) struct DeepseekAnthropicBackend {
    client: reqwest::Client,
    api_key: Option<SecretString>,
    base_url: String,
    model: String,
    max_output_tokens: Option<NonZeroU32>,
    thinking: Option<bool>,
    fallback: OpenAiCompatibleBackend,
}

impl DeepseekAnthropicBackend {
    fn endpoint_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(self.endpoint_url())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(body);
        if let Some(key) = &self.api_key {
            request = request.header("x-api-key", key.expose_secret());
        }
        request
    }

    fn request_body(&self, messages: &[Message], tools: &[Value], stream: bool) -> Value {
        let (system, anthropic_messages) = to_anthropic_messages(messages);
        let mut body = json!({
            "model": self.model,
            "max_tokens": self
                .max_output_tokens
                .map(NonZeroU32::get)
                .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
            "messages": anthropic_messages,
            "stream": stream,
        });
        if let Some(system) = system {
            body["system"] = json!(system);
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }
        match self.thinking {
            Some(true) => body["thinking"] = json!({"type": "enabled"}),
            Some(false) => body["thinking"] = json!({"type": "disabled"}),
            None => {}
        }
        body
    }

    async fn ask_streaming_once(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        let body = self.request_body(messages, tools, true);
        let messages_url = self.endpoint_url();
        let (res, attempt) = send_json(
            self.request(&body),
            &metadata,
            "deepseek-anthropic",
            &self.model,
        )
        .await?;
        let result = async {
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "DeepSeek Anthropic-compatible API error at {messages_url} (status {status}): {}",
                    rara_persistence::redaction::redact_secrets(body)
                ));
            }

            let mut stream = res.bytes_stream().eventsource();
            let mut blocks = AnthropicBlockAssembler::default();
            let mut stop_reason = None;
            let mut usage = None;

            while let Some(event) =
                next_stream_item_with_idle_timeout(&mut stream, "DeepSeek Anthropic SSE").await?
            {
                metadata.ensure_not_cancelled()?;
                let event = event.map_err(|error| anyhow!("Failed to decode SSE event: {error}"))?;
                let data = event.data.trim();
                if data.is_empty() {
                    continue;
                }
                let payload: Value = serde_json::from_str(data)
                    .map_err(|error| anyhow!("Failed to parse SSE payload: {error}"))?;
                record_usage(&attempt, payload.get("usage"));
                if let Some(message) = payload.get("message") {
                    record_usage(&attempt, message.get("usage"));
                }

                match payload.get("type").and_then(Value::as_str) {
                    Some("content_block_start") => blocks.start(&payload),
                    Some("content_block_delta") => {
                        if let Some(text) = blocks.delta_text(&payload) {
                            on_event(LlmStreamEvent::TextDelta(text));
                        }
                        if let Some(text) = blocks.delta_thinking(&payload) {
                            on_event(LlmStreamEvent::ReasoningDelta(text));
                        }
                    }
                    Some("content_block_stop") => blocks.stop(&payload),
                    Some("message_delta") => {
                        if let Some(reason) = payload
                            .get("delta")
                            .and_then(|delta| delta.get("stop_reason"))
                            .and_then(Value::as_str)
                        {
                            stop_reason = Some(reason.to_string());
                        }
                        if let Some(u) = payload.get("usage") {
                            usage = Some(u.clone());
                        }
                    }
                    Some("message_stop") => {
                        record_final_usage(&attempt, usage.as_ref());
                        break;
                    }
                    Some("error") => {
                        let message = payload
                            .get("error")
                            .and_then(|error| error.get("message"))
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error");
                        return Err(anyhow!("DeepSeek Anthropic-compatible stream error: {message}"));
                    }
                    _ => {}
                }
            }

            Ok(LlmResponse {
                content: blocks.into_content(),
                stop_reason,
                usage: usage.as_ref().map(parse_anthropic_token_usage),
            })
        }
        .await;
        finish_attempt(attempt, &result);
        result
    }
}

#[async_trait]
impl LlmBackend for DeepseekAnthropicBackend {
    fn model_label(&self) -> Option<String> {
        self.fallback.model_label()
    }

    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        self.ask_with_context(messages, tools, LlmTurnMetadata::default())
            .await
    }

    async fn ask_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
    ) -> Result<LlmResponse> {
        let mut ignored = |_event: LlmStreamEvent| {};
        self.ask_streaming_once(messages, tools, metadata, &mut ignored)
            .await
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask_streaming_once(messages, tools, metadata, on_event)
            .await
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        self.fallback.summarize(messages, instruction).await
    }

    async fn summarize_with_context(
        &self,
        messages: &[Message],
        instruction: &str,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.fallback
            .summarize_with_context(messages, instruction, metadata)
            .await
    }

    async fn classify_with_context(
        &self,
        instructions: &str,
        messages: &[Message],
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.fallback
            .classify_with_context(instructions, messages, metadata)
            .await
    }

    async fn summarize_with_prefix(
        &self,
        messages: &[Message],
        instruction: &str,
        prefix: &super::SummaryPrefix,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.fallback
            .summarize_with_prefix(messages, instruction, prefix, metadata)
            .await
    }

    fn context_budget(&self, messages: &[Message], tools: &[Value]) -> Option<ContextBudget> {
        self.fallback.context_budget(messages, tools)
    }

    fn cache_profile(&self) -> ProviderCacheProfile {
        self.fallback.cache_profile()
    }

    fn request_cache_fingerprint(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: &LlmTurnMetadata,
    ) -> Option<ModelRequestFingerprint> {
        self.fallback
            .request_cache_fingerprint(messages, tools, metadata)
    }
}

/// Accumulates one streamed Anthropic `content_block_*` sequence into our
/// internal [`ContentBlock`] vocabulary, in the order blocks close — which is
/// also their natural generation order, so no reordering is needed the way
/// `chat/completions` needs it for DeepSeek's inline DSML fallback.
#[derive(Default)]
struct AnthropicBlockAssembler {
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
    fn start(&mut self, payload: &Value) {
        let Some(index) = payload.get("index").and_then(Value::as_u64) else {
            return;
        };
        let block = payload.get("content_block");
        let pending = match block.and_then(|b| b.get("type")).and_then(Value::as_str) {
            Some("text") => PendingBlock::Text(String::new()),
            Some("thinking") => PendingBlock::Thinking {
                thinking: String::new(),
                signature: String::new(),
            },
            Some("tool_use") => PendingBlock::ToolUse {
                id: block
                    .and_then(|b| b.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: block
                    .and_then(|b| b.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                partial_json: String::new(),
            },
            _ => return,
        };
        self.in_progress.insert(index, pending);
    }

    fn delta_text(&mut self, payload: &Value) -> Option<String> {
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

    fn delta_thinking(&mut self, payload: &Value) -> Option<String> {
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

    fn stop(&mut self, payload: &Value) {
        let Some(index) = payload.get("index").and_then(Value::as_u64) else {
            return;
        };
        let Some(pending) = self.in_progress.remove(&index) else {
            return;
        };
        let block = match pending {
            PendingBlock::Text(text) => (!text.is_empty()).then(|| ContentBlock::Text { text }),
            PendingBlock::Thinking {
                thinking,
                signature,
            } => Some(ContentBlock::ProviderMetadata {
                provider: THINKING_PROVIDER.to_string(),
                key: THINKING_KEY.to_string(),
                value: json!({"thinking": thinking, "signature": signature}),
            }),
            PendingBlock::ToolUse {
                id,
                name,
                partial_json,
            } => Some(ContentBlock::ToolUse {
                id,
                name,
                input: parse_tool_use_input(&partial_json),
            }),
        };
        if let Some(block) = block {
            self.finished.push(block);
        }
    }

    fn into_content(self) -> Vec<ContentBlock> {
        self.finished
    }
}

fn parse_tool_use_input(partial_json: &str) -> Value {
    let trimmed = partial_json.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({}))
}

fn parse_anthropic_token_usage(usage: &Value) -> TokenUsage {
    let field = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0) as u32;
    TokenUsage {
        input_tokens: field("input_tokens"),
        output_tokens: field("output_tokens"),
        cache_hit_tokens: field("cache_read_input_tokens"),
        cache_miss_tokens: field("cache_creation_input_tokens"),
    }
}

fn to_anthropic_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut out = Vec::new();
    for message in messages {
        if message.role == "system" {
            if let Some(text) = message.content.as_str().map(str::to_string)
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
                if item.get("provider").and_then(Value::as_str) == Some(THINKING_PROVIDER)
                    && item.get("key").and_then(Value::as_str) == Some(THINKING_KEY) =>
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
    fn base_url_only_activates_for_deepseeks_own_host() {
        assert_eq!(
            deepseek_anthropic_base_url("https://api.deepseek.com/v1"),
            Some("https://api.deepseek.com/anthropic".to_string())
        );
        assert_eq!(
            deepseek_anthropic_base_url("https://my-gateway.internal/v1"),
            None
        );
        assert_eq!(deepseek_anthropic_base_url("not a url"), None);
    }

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
    fn block_assembler_separates_thinking_text_and_tool_use_by_construction() {
        let mut assembler = AnthropicBlockAssembler::default();
        assembler.start(&json!({"index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}));
        assert_eq!(
            assembler.delta_thinking(
                &json!({"index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}})
            ),
            Some("hmm".to_string())
        );
        assembler.delta_thinking(
            &json!({"index": 0, "delta": {"type": "signature_delta", "signature": "sig-1"}}),
        );
        assembler.stop(&json!({"index": 0}));

        assembler.start(&json!({"index": 1, "content_block": {"type": "text", "text": ""}}));
        assert_eq!(
            assembler.delta_text(
                &json!({"index": 1, "delta": {"type": "text_delta", "text": "Let me check.\n"}})
            ),
            Some("Let me check.\n".to_string())
        );
        assembler.stop(&json!({"index": 1}));

        assembler.start(&json!({"index": 2, "content_block": {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {}}}));
        for chunk in ["{", "\"city\"", ":", "\"Paris\"", "}"] {
            assembler.delta_thinking(
                &json!({"index": 2, "delta": {"type": "input_json_delta", "partial_json": chunk}}),
            );
        }
        assembler.stop(&json!({"index": 2}));

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
