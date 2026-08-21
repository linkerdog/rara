mod cache_observation;
mod codex;
mod protocol;
mod usage;

use std::borrow::Cow;
use std::fmt;
use std::num::NonZeroU32;
use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use backon::{ExponentialBuilder, Retryable};
use eventsource_stream::Eventsource;
use rara_persistence::redaction::{redact_secrets, sanitize_url_for_display};
use reqwest::{Response, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use uuid::Uuid;

use self::cache_observation::{
    apply_deepseek_user_id, enable_streaming_usage, fingerprint_request,
};
#[cfg(test)]
pub(super) use self::protocol::to_openai_messages;
pub(super) use self::protocol::{
    build_chat_completion_request_body, build_streaming_response_content,
    is_openai_stream_idle_error, merge_streaming_tool_calls, parse_chat_completion_response,
    to_openai_messages_for_endpoint,
};
use self::usage::parse_openai_token_usage;
use super::shared::{
    ContextBudget, LlmBackend, LlmStreamEvent, LlmTurnMetadata, ProviderCacheProfile,
    context_budget_from_window, extract_message_text, http_client_for_target,
    is_retryable_http_error, model_context_budget, next_stream_item_with_idle_timeout,
    retry_send_json,
};
use crate::agent::Message;
use crate::config::OpenAiEndpointKind;
use crate::control_tokens::{has_deepseek_control_evidence, scrub_deepseek_visible_text};
use crate::llm::{ContentBlock, LlmResponse};
use crate::model_observation::ModelRequestFingerprint;

const STREAM_IDLE_RETRY_ATTEMPTS: usize = 1;
const DEEPSEEK_THINK_OPEN: &str = "<think>";

#[derive(Debug)]
pub(crate) struct OpenAiApiError {
    status: Option<StatusCode>,
    sanitized_body: String,
    error_type: Option<String>,
    error_code: Option<String>,
}

#[derive(Default)]
pub(super) struct DeepseekTextStreamScrubber {
    state: DeepseekThinkStreamState,
    prefix_buffer: String,
    leading_think_is_control: bool,
}

#[derive(Default)]
enum DeepseekThinkStreamState {
    #[default]
    AtStart,
    PendingLeadingThink,
    Done,
}

impl DeepseekTextStreamScrubber {
    pub(super) fn new(leading_think_is_control: bool) -> Self {
        Self {
            leading_think_is_control,
            ..Self::default()
        }
    }

    pub(super) fn push(&mut self, delta: &str) -> String {
        match self.state {
            DeepseekThinkStreamState::AtStart => self.push_at_start(delta),
            DeepseekThinkStreamState::PendingLeadingThink => {
                self.prefix_buffer.push_str(delta);
                self.flush_pending_leading_think_if_ready()
            }
            DeepseekThinkStreamState::Done => delta.to_string(),
        }
    }

    fn push_at_start(&mut self, delta: &str) -> String {
        self.prefix_buffer.push_str(delta);
        let trimmed_prefix = self.prefix_buffer.trim_start();
        if DEEPSEEK_THINK_OPEN.starts_with(trimmed_prefix) {
            return String::new();
        }
        if !trimmed_prefix.starts_with(DEEPSEEK_THINK_OPEN) {
            self.state = DeepseekThinkStreamState::Done;
            return std::mem::take(&mut self.prefix_buffer);
        }

        self.state = DeepseekThinkStreamState::PendingLeadingThink;
        self.flush_pending_leading_think_if_ready()
    }

    fn flush_pending_leading_think_if_ready(&mut self) -> String {
        if !self.leading_think_is_control && !has_deepseek_control_evidence(&self.prefix_buffer) {
            return String::new();
        }
        if !self.prefix_buffer.contains("</think>") {
            return String::new();
        }
        self.finish()
    }

    pub(super) fn finish(&mut self) -> String {
        let buffered = std::mem::take(&mut self.prefix_buffer);
        self.state = DeepseekThinkStreamState::Done;
        if buffered.is_empty() {
            return String::new();
        }
        let has_control_evidence =
            self.leading_think_is_control || has_deepseek_control_evidence(&buffered);
        scrub_deepseek_visible_text(&buffered, has_control_evidence)
    }
}

impl OpenAiApiError {
    #[cfg(test)]
    pub(crate) fn for_test(
        status: Option<StatusCode>,
        error_type: Option<&str>,
        error_code: Option<&str>,
    ) -> Self {
        Self {
            status,
            sanitized_body: "{}".to_string(),
            error_type: error_type.map(ToString::to_string),
            error_code: error_code.map(ToString::to_string),
        }
    }

    pub(crate) fn from_response_failed_error(error: &Value) -> Self {
        let sanitized_body = redact_secrets(error.to_string());
        Self {
            status: None,
            sanitized_body,
            error_type: error
                .get("type")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            error_code: error
                .get("code")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        }
    }
}

impl fmt::Display for OpenAiApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(status) = self.status {
            write!(f, "API Error (status {status}): {}", self.sanitized_body)
        } else {
            write!(f, "API Error: {}", self.sanitized_body)
        }
    }
}

impl std::error::Error for OpenAiApiError {}

#[cfg(test)]
pub(crate) use self::codex::{
    apply_codex_stream_event, build_codex_responses_request, build_codex_stream_response,
    parse_codex_response, to_codex_input_items,
};

pub struct OpenAiCompatibleBackend {
    pub client: reqwest::Client,
    pub api_key: Option<SecretString>,
    pub base_url: String,
    pub context_window_override: Option<usize>,
    pub model: String,
    pub auxiliary_model: Option<String>,
    pub endpoint_kind: OpenAiEndpointKind,
    pub reasoning_effort: Option<String>,
    pub thinking: Option<bool>,
    max_output_tokens: Option<NonZeroU32>,
    deepseek_user_id: Option<String>,
    request_fingerprint_scope: String,
    request_fingerprint_salt: [u8; 16],
}

impl OpenAiCompatibleBackend {
    pub fn new(api_key: Option<SecretString>, base_url: String, model: String) -> Result<Self> {
        Self::new_with_endpoint_kind(api_key, base_url, model, OpenAiEndpointKind::Custom)
    }

    pub fn new_with_endpoint_kind(
        api_key: Option<SecretString>,
        base_url: String,
        model: String,
        endpoint_kind: OpenAiEndpointKind,
    ) -> Result<Self> {
        Self::new_with_endpoint_kind_and_reasoning(
            api_key,
            base_url,
            model,
            endpoint_kind,
            None,
            None,
        )
    }

    pub fn new_with_endpoint_kind_and_reasoning(
        api_key: Option<SecretString>,
        base_url: String,
        model: String,
        endpoint_kind: OpenAiEndpointKind,
        reasoning_effort: Option<String>,
        thinking: Option<bool>,
    ) -> Result<Self> {
        let fingerprint_scope = Uuid::new_v4();
        let fingerprint_salt = Uuid::new_v4();
        Ok(Self {
            client: http_client_for_target(&base_url)?,
            api_key,
            base_url,
            context_window_override: None,
            model,
            auxiliary_model: None,
            endpoint_kind,
            reasoning_effort,
            thinking,
            max_output_tokens: None,
            deepseek_user_id: None,
            request_fingerprint_scope: fingerprint_scope.to_string(),
            request_fingerprint_salt: *fingerprint_salt.as_bytes(),
        })
    }

    pub fn with_auxiliary_model(mut self, auxiliary_model: Option<String>) -> Self {
        self.auxiliary_model = auxiliary_model
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty());
        self
    }

    /// Bound provider output tokens for callers such as opt-in measurement tools.
    pub fn with_max_output_tokens(mut self, max_output_tokens: NonZeroU32) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }

    pub(crate) fn with_deepseek_user_id(mut self, user_id: String) -> Self {
        self.deepseek_user_id = Some(user_id);
        self
    }

    fn chat_completion_request_body(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
    ) -> Value {
        let mut body = build_chat_completion_request_body(
            model,
            messages,
            tools,
            self.endpoint_kind,
            self.reasoning_effort.as_deref(),
            self.thinking,
            metadata,
        );
        if let Some(max_output_tokens) = self.max_output_tokens {
            body["max_tokens"] = json!(max_output_tokens.get());
        }
        apply_deepseek_user_id(
            &mut body,
            self.endpoint_kind,
            self.deepseek_user_id.as_deref(),
        );
        body
    }

    fn endpoint_url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let normalized_base = if base.ends_with("/v1") {
            base.to_string()
        } else {
            format!("{base}/v1")
        };
        format!("{normalized_base}/{path}")
    }

    async fn ask_streaming_once(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        let mut body =
            self.chat_completion_request_body(&self.model, messages, tools, metadata.clone());
        enable_streaming_usage(&mut body, self.endpoint_kind);

        let completions_url = self.endpoint_url("chat/completions");
        let api_key = self.api_key.as_ref().map(|k| k.expose_secret());
        let res = retry_send_json(&self.client, &completions_url, &body, api_key).await?;

        if !res.status().is_success() {
            return Err(anyhow!(
                "API Error at {}: {}",
                sanitize_url_for_display(&completions_url),
                redact_secrets(res.text().await?)
            ));
        }

        let mut stream = res.bytes_stream().eventsource();
        let mut streamed_text = String::new();
        let mut streamed_reasoning_content = String::new();
        let mut streamed_tool_calls: Vec<Value> = Vec::new();
        let mut deepseek_text_scrubber = (self.endpoint_kind == OpenAiEndpointKind::Deepseek)
            .then(|| DeepseekTextStreamScrubber::new(self.thinking == Some(true)));
        let mut stop_reason = None;
        let mut usage = None;

        while let Some(event) =
            next_stream_item_with_idle_timeout(&mut stream, "OpenAI-compatible SSE").await?
        {
            metadata.ensure_not_cancelled()?;
            let event = event.map_err(|error| anyhow!("Failed to decode SSE event: {error}"))?;
            let data = event.data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                break;
            }
            let payload: Value = serde_json::from_str(data)
                .map_err(|error| anyhow!("Failed to parse SSE payload: {error}"))?;

            if let Some(choice) = payload
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
            {
                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(Value::as_str)
                        && !content.is_empty()
                    {
                        if let Some(scrubber) = deepseek_text_scrubber.as_mut() {
                            let visible = scrubber.push(content);
                            if !visible.is_empty() {
                                on_event(LlmStreamEvent::TextDelta(visible));
                            }
                        } else {
                            on_event(LlmStreamEvent::TextDelta(content.to_string()));
                        }
                        streamed_text.push_str(content);
                    }
                    if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str)
                    {
                        if !reasoning.is_empty() {
                            on_event(LlmStreamEvent::ReasoningDelta(reasoning.to_string()));
                        }
                        streamed_reasoning_content.push_str(reasoning);
                    }
                    if let Some(tool_deltas) = delta.get("tool_calls").and_then(Value::as_array) {
                        merge_streaming_tool_calls(&mut streamed_tool_calls, tool_deltas)?;
                    }
                }
                if let Some(finish) = choice.get("finish_reason").and_then(Value::as_str) {
                    stop_reason = Some(finish.to_string());
                }
            }
            if let Some(u) = payload.get("usage")
                && !u.is_null()
            {
                usage = Some(u.clone());
            }
        }

        if let Some(scrubber) = deepseek_text_scrubber.as_mut() {
            let visible = scrubber.finish();
            if !visible.is_empty() {
                on_event(LlmStreamEvent::TextDelta(visible));
            }
        }

        let content = build_streaming_response_content(
            self.endpoint_kind,
            streamed_text,
            streamed_reasoning_content,
            &streamed_tool_calls,
        )?;

        Ok(LlmResponse {
            content,
            stop_reason,
            usage: usage.as_ref().map(parse_openai_token_usage),
        })
    }
}

/// Fetch the model's context window size from GET /v1/models.
/// Falls back to None if the API call fails or the model is not found.
pub async fn fetch_model_context_window(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&SecretString>,
    model: &str,
) -> Option<usize> {
    let base = base_url.trim_end_matches('/');
    let url = if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    };

    let res = (|| async {
        let mut req = client.get(&url);
        if let Some(key) = api_key {
            req = req.bearer_auth(key.expose_secret());
        }
        let res = req
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| anyhow!(e))?;
        let status = res.status();
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            return Err(anyhow!("HTTP {}", status.as_u16()));
        }
        Ok(res)
    })
    .retry(ExponentialBuilder::default().with_jitter())
    .when(|e: &anyhow::Error| is_retryable_http_error(e))
    .await
    .ok()?
    .error_for_status()
    .ok()?;
    let body: Value = res.json().await.ok()?;
    let models = body["data"].as_array()?;

    for m in models {
        if m["id"].as_str() == Some(model) {
            return m["context_length"]
                .as_u64()
                .and_then(|tokens| usize::try_from(tokens).ok())
                .filter(|tokens| *tokens > 0);
        }
    }
    None
}

#[async_trait]
impl LlmBackend for OpenAiCompatibleBackend {
    fn model_label(&self) -> Option<String> {
        Some(self.model.clone())
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
        let body =
            self.chat_completion_request_body(&self.model, messages, tools, metadata.clone());

        let completions_url = self.endpoint_url("chat/completions");
        let api_key = self.api_key.as_ref().map(|k| k.expose_secret());
        let res = retry_send_json(&self.client, &completions_url, &body, api_key).await?;

        if !res.status().is_success() {
            return Err(anyhow!(
                "API Error at {}: {}",
                sanitize_url_for_display(&completions_url),
                redact_secrets(res.text().await?)
            ));
        }
        let resp_json: Value = res.json().await?;
        parse_chat_completion_response(&resp_json, self.endpoint_kind)
    }

    async fn ask_streaming_with_context(
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
                    if attempts < STREAM_IDLE_RETRY_ATTEMPTS
                        && !emitted_delta
                        && is_openai_stream_idle_error(&error) =>
                {
                    attempts += 1;
                    metadata.ensure_not_cancelled()?;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        let mut msgs = messages.to_vec();
        msgs.push(Message {
            role: "user".to_string(),
            content: json!(instruction),
        });
        let summary_model = self.summary_model();
        let summary = self
            .summarize_with_model(summary_model.as_ref(), &msgs)
            .await;
        if summary_model.as_ref() != self.model.as_str()
            && summary
                .as_ref()
                .is_err_and(is_auxiliary_model_retryable_error)
        {
            return self.summarize_with_model(self.model.as_str(), &msgs).await;
        }
        summary
    }

    fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
        let main_budget = self
            .context_window_override
            .map(context_budget_from_window)
            .or_else(|| model_context_budget(self.model.as_str()));
        let summary_model = self.summary_model();
        let summary_budget = if summary_model.as_ref() == self.model.as_str() {
            main_budget
        } else {
            model_context_budget(summary_model.as_ref()).or(main_budget)
        };
        match (main_budget, summary_budget) {
            (Some(main), Some(summary)) => {
                if summary.context_window_tokens < main.context_window_tokens {
                    Some(summary)
                } else {
                    Some(main)
                }
            }
            (Some(main), None) => Some(main),
            (None, Some(summary)) => Some(summary),
            (None, None) => None,
        }
    }

    fn cache_profile(&self) -> ProviderCacheProfile {
        match self.endpoint_kind {
            OpenAiEndpointKind::Deepseek => {
                ProviderCacheProfile::automatic_prefix_cache_with_usage()
            }
            OpenAiEndpointKind::Custom
            | OpenAiEndpointKind::Kimi
            | OpenAiEndpointKind::KimiCoding
            | OpenAiEndpointKind::Openrouter => ProviderCacheProfile::none(),
        }
    }

    fn request_cache_fingerprint(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: &LlmTurnMetadata,
    ) -> Option<ModelRequestFingerprint> {
        (self.endpoint_kind == OpenAiEndpointKind::Deepseek).then(|| {
            fingerprint_request(
                &self.chat_completion_request_body(&self.model, messages, tools, metadata.clone()),
                &self.request_fingerprint_scope,
                &self.request_fingerprint_salt,
            )
        })
    }
}

impl OpenAiCompatibleBackend {
    async fn summarize_with_model(&self, model: &str, msgs: &[Message]) -> Result<String> {
        let body = self.chat_completion_request_body(model, msgs, &[], LlmTurnMetadata::default());
        let completions_url = self.endpoint_url("chat/completions");
        let api_key = self.api_key.as_ref().map(|k| k.expose_secret());
        let res = retry_send_json(&self.client, &completions_url, &body, api_key).await?;
        if !res.status().is_success() {
            return Err(api_error_from_response(res)
                .await
                .context_url(&completions_url));
        }
        let resp_json: Value = res.json().await?;
        Ok(
            extract_message_text(resp_json["choices"][0]["message"].get("content"))
                .unwrap_or_default(),
        )
    }
    fn summary_model(&self) -> Cow<'_, str> {
        self.auxiliary_model
            .as_deref()
            .map(Cow::Borrowed)
            .or_else(|| infer_openai_compatible_auxiliary_model(&self.model, self.endpoint_kind))
            .unwrap_or(Cow::Borrowed(self.model.as_str()))
    }
}

trait OpenAiApiErrorContext {
    fn context_url(self, url: &str) -> anyhow::Error;
}

impl OpenAiApiErrorContext for OpenAiApiError {
    fn context_url(self, url: &str) -> anyhow::Error {
        anyhow::Error::new(self).context(format!("API Error at {}", sanitize_url_for_display(url)))
    }
}

pub(super) async fn api_error_from_response(response: Response) -> OpenAiApiError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let sanitized_body = redact_secrets(body);
    let parsed = serde_json::from_str::<Value>(&sanitized_body).ok();
    let error = parsed.as_ref().and_then(|value| value.get("error"));
    OpenAiApiError {
        status: Some(status),
        sanitized_body,
        error_type: error
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        error_code: error
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
    }
}

pub(crate) fn is_auxiliary_model_unsupported_error(error: &anyhow::Error) -> bool {
    let Some(api_error) = error.downcast_ref::<OpenAiApiError>() else {
        return false;
    };
    api_error.status == Some(StatusCode::NOT_FOUND)
        || matches!(
            api_error.error_code.as_deref(),
            Some("model_not_found" | "model_not_supported" | "invalid_model")
        )
        || matches!(
            api_error.error_type.as_deref(),
            Some("model_not_found" | "model_not_supported" | "invalid_model")
        )
}

pub(crate) fn is_auxiliary_model_retryable_error(error: &anyhow::Error) -> bool {
    is_auxiliary_model_unsupported_error(error) || is_context_window_error(error)
}

pub(crate) fn is_context_window_error(error: &anyhow::Error) -> bool {
    let Some(api_error) = error.downcast_ref::<OpenAiApiError>() else {
        return false;
    };
    api_error.error_code.as_deref() == Some("context_length_exceeded")
}

#[cfg(test)]
pub(crate) fn context_window_error_for_test() -> anyhow::Error {
    anyhow::Error::new(OpenAiApiError::for_test(
        Some(StatusCode::BAD_REQUEST),
        Some("invalid_request_error"),
        Some("context_length_exceeded"),
    ))
}

pub(crate) fn infer_openai_compatible_auxiliary_model(
    model: &str,
    endpoint_kind: OpenAiEndpointKind,
) -> Option<Cow<'_, str>> {
    if endpoint_kind != OpenAiEndpointKind::Deepseek {
        return None;
    }
    infer_deepseek_lite_model(model)
}

fn infer_deepseek_lite_model(model: &str) -> Option<Cow<'_, str>> {
    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.contains("deepseek") || lower.contains("flash") {
        return None;
    }
    if lower.contains("v4") && lower.ends_with("-pro") {
        let prefix = &trimmed[..trimmed.len().saturating_sub("-pro".len())];
        return Some(Cow::Owned(format!("{prefix}-flash")));
    }
    None
}

pub struct CodexBackend {
    reasoning_effort: Option<String>,
    client: reqwest::Client,
    api_key: Option<SecretString>,
    base_url: String,
    model: String,
    auxiliary_model: Option<String>,
}
