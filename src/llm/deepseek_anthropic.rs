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
//! [`messages::to_anthropic_messages`] reconstructs it from the
//! `ContentBlock::ProviderMetadata` slot this backend writes on responses.

mod messages;
mod stream;

use std::num::NonZeroU32;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};

use self::messages::to_anthropic_messages;
use self::stream::{AnthropicBlockAssembler, merge_usage, parse_anthropic_token_usage};
use super::inference_transport::{finish_attempt, record_final_usage, record_usage, send_json};
use super::openai_compatible::{OpenAiCompatibleBackend, fingerprint_request};
use super::shared::{
    ContextBudget, LlmBackend, LlmStreamEvent, LlmTurnMetadata, ProviderCacheProfile,
    http_client_for_target, next_stream_item_with_idle_timeout,
};
use crate::agent::Message;
use crate::llm::LlmResponse;
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

/// Only activate the Anthropic-compatible path against exactly DeepSeek's
/// own official endpoint shape: HTTPS, the default port, no credentials or
/// query/fragment, and a root or `/v1` path. The `/anthropic` mount is a
/// DeepSeek-specific convention that a differently-shaped URL on the same
/// host (a tenant-specific path, a proxy, a non-default port) cannot be
/// assumed to also expose.
fn deepseek_anthropic_base_url(configured_base_url: &str) -> Option<String> {
    let url = url::Url::parse(configured_base_url).ok()?;
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if !url.host_str()?.eq_ignore_ascii_case("api.deepseek.com") {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    (path.is_empty() || path == "/v1").then(|| "https://api.deepseek.com/anthropic".to_string())
}

/// Settings this backend needs beyond `base_url`/`model`/`kind`, bundled so
/// callers don't have to thread them through as separate parameters.
/// `max_output_tokens`/`temperature`/`top_p` come from the wrapped
/// [`OpenAiCompatibleBackend`]'s own construction inputs, which it exposes
/// no getters for, so registry-model callers carry them in directly instead.
#[derive(Default)]
pub(crate) struct DeepseekAnthropicConfig {
    pub(crate) api_key: Option<SecretString>,
    pub(crate) thinking: Option<bool>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) max_output_tokens: Option<NonZeroU32>,
    pub(crate) temperature: Option<f64>,
    pub(crate) top_p: Option<f64>,
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
    config: DeepseekAnthropicConfig,
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
    let request_fingerprint_scope = uuid::Uuid::new_v4();
    let request_fingerprint_salt = uuid::Uuid::new_v4();
    Box::new(DeepseekAnthropicBackend {
        client,
        api_key: config.api_key,
        base_url: anthropic_base_url,
        model: model.to_string(),
        max_output_tokens: config.max_output_tokens,
        temperature: config.temperature,
        top_p: config.top_p,
        thinking: config.thinking,
        reasoning_effort: config.reasoning_effort,
        billing_provider: fallback.billing_provider(),
        request_fingerprint_scope: request_fingerprint_scope.to_string(),
        request_fingerprint_salt: *request_fingerprint_salt.as_bytes(),
        fallback,
    })
}

pub(crate) struct DeepseekAnthropicBackend {
    client: reqwest::Client,
    api_key: Option<SecretString>,
    base_url: String,
    model: String,
    max_output_tokens: Option<NonZeroU32>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    thinking: Option<bool>,
    reasoning_effort: Option<String>,
    /// Matches `fallback`'s own billing identity so streamed Anthropic-path
    /// attempts price against the same provider/model tariff as
    /// `chat/completions`, rather than reporting as unpriced.
    billing_provider: String,
    /// Own scope/salt (distinct from `fallback`'s, which has no getter):
    /// fingerprints must be built from this backend's own Anthropic-shaped
    /// request body, not `fallback`'s OpenAI-compatible one, since a
    /// fingerprint from a body that is never actually sent would make
    /// cache-locality reports meaningless for this path.
    request_fingerprint_scope: String,
    request_fingerprint_salt: [u8; 16],
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
        // Only send an effort when thinking isn't explicitly turned off —
        // pairing `thinking: disabled` with a non-off `output_config.effort`
        // is a contradictory request the way chat/completions never sends
        // effort at all for a thinking-disabled turn.
        if self.thinking != Some(false)
            && let Some(effort) = self
                .reasoning_effort
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        {
            // Matches the DeepSeek reference harness's request shape
            // (`output_config.effort`); DeepSeek's chat/completions endpoint
            // never applies effort to `deepseek-flash` at all (its gate
            // requires "v4"/"reasoner" in the model name), so there is no
            // existing normalization to reuse here. Verified live that
            // DeepSeek's Anthropic-compatible endpoint accepts RARA's
            // unnormalized effort vocabulary (e.g. "medium", "xhigh")
            // without erroring.
            body["output_config"] = json!({"effort": effort.to_ascii_lowercase()});
        }
        if let Some(value) = self.temperature {
            body["temperature"] = json!(value);
        }
        if let Some(value) = self.top_p {
            body["top_p"] = json!(value);
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
            &self.billing_provider,
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
                    Some("message_start") => {
                        if let Some(u) = payload.get("message").and_then(|m| m.get("usage")) {
                            usage = Some(merge_usage(usage.take(), u));
                        }
                    }
                    Some("content_block_start") => {
                        let (initial_text, initial_thinking) = blocks.start(&payload)?;
                        if let Some(text) = initial_text {
                            on_event(LlmStreamEvent::TextDelta(text));
                        }
                        if let Some(text) = initial_thinking {
                            on_event(LlmStreamEvent::ReasoningDelta(text));
                        }
                    }
                    Some("content_block_delta") => {
                        if let Some(text) = blocks.delta_text(&payload) {
                            on_event(LlmStreamEvent::TextDelta(text));
                        }
                        if let Some(text) = blocks.delta_thinking(&payload) {
                            on_event(LlmStreamEvent::ReasoningDelta(text));
                        }
                    }
                    Some("content_block_stop") => blocks.stop(&payload)?,
                    Some("message_delta") => {
                        if let Some(reason) = payload
                            .get("delta")
                            .and_then(|delta| delta.get("stop_reason"))
                            .and_then(Value::as_str)
                        {
                            stop_reason = Some(reason.to_string());
                        }
                        if let Some(u) = payload.get("usage") {
                            usage = Some(merge_usage(usage.take(), u));
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

    /// Retries once on a stream that idled out before emitting any delta,
    /// same as `OpenAiCompatibleBackend::ask_streaming_with_context` — this
    /// backend routes through `ask_streaming_once` for both the streaming
    /// and non-streaming trait methods, so both need this protection rather
    /// than failing a `deepseek-flash` turn outright on a transient stall.
    async fn ask_streaming_with_retry(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        let mut attempts = 0usize;
        loop {
            let mut emitted_delta = false;
            let mut relay_event = |event: LlmStreamEvent| {
                emitted_delta = true;
                on_event(event);
            };
            let result = self
                .ask_streaming_once(messages, tools, metadata.clone(), &mut relay_event)
                .await;
            match result {
                Ok(response) => return Ok(response),
                Err(error)
                    if attempts < super::openai_compatible::STREAM_IDLE_RETRY_ATTEMPTS
                        && !emitted_delta
                        && super::openai_compatible::is_openai_stream_idle_error(&error) =>
                {
                    attempts += 1;
                    metadata.ensure_not_cancelled()?;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
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
        self.ask_streaming_with_retry(messages, tools, metadata, &mut ignored)
            .await
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask_streaming_with_retry(messages, tools, metadata, on_event)
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
        _metadata: &LlmTurnMetadata,
    ) -> Option<ModelRequestFingerprint> {
        self.fallback
            .cache_profile()
            .cache_usage_accounting
            .then(|| {
                fingerprint_request(
                    &self.request_body(messages, tools, true),
                    &self.request_fingerprint_scope,
                    &self.request_fingerprint_salt,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_backend(
        thinking: Option<bool>,
        reasoning_effort: Option<&str>,
    ) -> DeepseekAnthropicBackend {
        DeepseekAnthropicBackend {
            client: reqwest::Client::new(),
            api_key: None,
            base_url: "https://api.deepseek.com/anthropic".to_string(),
            model: DEEPSEEK_ANTHROPIC_MODEL.to_string(),
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            thinking,
            reasoning_effort: reasoning_effort.map(str::to_string),
            billing_provider: "DeepSeek".to_string(),
            request_fingerprint_scope: "test-scope".to_string(),
            request_fingerprint_salt: [0u8; 16],
            fallback: OpenAiCompatibleBackend::new(
                None,
                "https://api.deepseek.com/v1".to_string(),
                DEEPSEEK_ANTHROPIC_MODEL.to_string(),
            )
            .expect("fallback backend construction cannot fail for a fixed valid URL"),
        }
    }

    #[test]
    fn request_body_never_sends_effort_when_thinking_is_explicitly_disabled() {
        let backend = test_backend(Some(false), Some("high"));
        let body = backend.request_body(&[], &[], false);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert!(
            body.get("output_config").is_none(),
            "a disabled-thinking request must not also carry a contradictory effort"
        );
    }

    #[test]
    fn request_body_sends_effort_when_thinking_is_enabled_or_unset() {
        let enabled = test_backend(Some(true), Some("HIGH"));
        assert_eq!(
            enabled.request_body(&[], &[], false)["output_config"],
            json!({"effort": "high"})
        );

        let unset = test_backend(None, Some("max"));
        assert_eq!(
            unset.request_body(&[], &[], false)["output_config"],
            json!({"effort": "max"})
        );
    }

    #[test]
    fn base_url_activates_only_for_deepseeks_exact_official_url_shape() {
        for accepted in [
            "https://api.deepseek.com",
            "https://api.deepseek.com/",
            "https://api.deepseek.com/v1",
            "https://api.deepseek.com/v1/",
            "https://API.DeepSeek.com/v1",
            "https://api.deepseek.com:443/v1",
        ] {
            assert_eq!(
                deepseek_anthropic_base_url(accepted),
                Some("https://api.deepseek.com/anthropic".to_string()),
                "expected {accepted} to activate"
            );
        }
        for rejected in [
            "https://my-gateway.internal/v1",
            "not a url",
            "https://api.deepseek.com/proxy/v1",
            "http://api.deepseek.com/v1",
            "https://api.deepseek.com:8443/v1",
            "https://user:pass@api.deepseek.com/v1",
            "https://api.deepseek.com/v1?foo=bar",
            "https://api.deepseek.com/v1#frag",
        ] {
            assert_eq!(
                deepseek_anthropic_base_url(rejected),
                None,
                "expected {rejected} to stay on chat/completions"
            );
        }
    }
}
