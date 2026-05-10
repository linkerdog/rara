use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use backon::{ExponentialBuilder, Retryable};
use futures::{Stream, StreamExt};
use serde_json::{Value, json};
use url::Url;

use crate::agent::Message;
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AssistantToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub context_window_tokens: usize,
    pub reserved_output_tokens: usize,
    pub compact_threshold_tokens: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderCacheProfile {
    pub automatic_prefix_cache: bool,
    pub cache_usage_accounting: bool,
    pub cache_edit: bool,
    pub cache_retention_control: bool,
}

impl ProviderCacheProfile {
    pub const fn none() -> Self {
        Self {
            automatic_prefix_cache: false,
            cache_usage_accounting: false,
            cache_edit: false,
            cache_retention_control: false,
        }
    }

    pub const fn automatic_prefix_cache_with_usage() -> Self {
        Self {
            automatic_prefix_cache: true,
            cache_usage_accounting: true,
            cache_edit: false,
            cache_retention_control: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmExecutionMode {
    Execute,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmStreamEvent {
    TextDelta(String),
    ReasoningDelta(String),
}

#[derive(Debug, Clone)]
pub struct LlmTurnMetadata {
    execution_mode: LlmExecutionMode,
    cancellation: Option<Arc<AtomicBool>>,
}

impl Default for LlmTurnMetadata {
    fn default() -> Self {
        Self {
            execution_mode: LlmExecutionMode::Execute,
            cancellation: None,
        }
    }
}

impl LlmTurnMetadata {
    pub fn execute() -> Self {
        Self {
            execution_mode: LlmExecutionMode::Execute,
            cancellation: None,
        }
    }

    pub fn plan() -> Self {
        Self {
            execution_mode: LlmExecutionMode::Plan,
            cancellation: None,
        }
    }

    pub fn with_cancellation(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|cancellation| cancellation.load(Ordering::SeqCst))
    }

    pub fn ensure_not_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(anyhow!("LLM turn cancelled by user"));
        }
        Ok(())
    }

    pub fn prefers_strong_reasoning(&self) -> bool {
        matches!(self.execution_mode, LlmExecutionMode::Plan)
    }
}

#[async_trait]
pub trait LlmBackend: Send + Sync {
    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse>;
    async fn ask_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        _metadata: LlmTurnMetadata,
    ) -> Result<LlmResponse> {
        self.ask(messages, tools).await
    }

    async fn ask_streaming(
        &self,
        messages: &[Message],
        tools: &[Value],
        _on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask(messages, tools).await
    }
    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        _metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask_streaming(messages, tools, on_event).await
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String>;
    /// Side-channel classifier call (auto-permission, background task status).
    ///
    /// Default implementation prepends an instructions message and delegates to
    /// `summarize()` via the auxiliary model,
    /// but backends may override to use a dedicated classifier endpoint or model.
    async fn classify(&self, instructions: &str, messages: &[Message]) -> Result<String> {
        let mut classify_msgs = vec![Message {
            role: "system".into(),
            content: serde_json::Value::String(instructions.into()),
        }];
        classify_msgs.extend(messages.iter().cloned());
        self.summarize(&classify_msgs, instructions).await
    }

    fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
        None
    }
    fn cache_profile(&self) -> ProviderCacheProfile {
        ProviderCacheProfile::none()
    }
}

pub struct MockLlm;

#[async_trait]
impl LlmBackend for MockLlm {
    async fn ask(&self, messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        let last_msg_json = &messages.last().unwrap().content;
        let last_msg_text = if last_msg_json.is_array() {
            last_msg_json
                .get(0)
                .and_then(|b| b.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
        } else {
            last_msg_json.as_str().unwrap_or("")
        };
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: format!("Mock Response: {}", last_msg_text),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                ..TokenUsage::default()
            }),
        })
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1; 128])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("Mock summary".into())
    }
}

pub(super) fn render_openai_message_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(items) = content.as_array() {
        let text_parts = items
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item.get("text").and_then(Value::as_str).map(str::to_string),
                Some("tool_result") => item
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|text| format!("tool_result: {text}")),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !text_parts.is_empty() {
            return text_parts.join("\n\n");
        }
    }
    content.to_string()
}

pub(super) fn extract_message_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let items = content.as_array()?;
    let texts = items
        .iter()
        .filter_map(|item| {
            let item_type = item.get("type").and_then(Value::as_str)?;
            if item_type == "text" {
                item.get("text").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n\n"))
    }
}

pub(super) fn collect_assistant_content(content: &Value) -> (Vec<String>, Vec<AssistantToolUse>) {
    let mut text_parts = Vec::new();
    let mut tool_uses = Vec::new();

    if let Some(items) = content.as_array() {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str)
                        && !text.trim().is_empty()
                    {
                        text_parts.push(text.to_string());
                    }
                }
                Some("tool_use") => {
                    tool_uses.push(AssistantToolUse {
                        id: item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input: item.get("input").cloned().unwrap_or_else(|| json!({})),
                    });
                }
                _ => {}
            }
        }
    } else if let Some(text) = content.as_str() {
        text_parts.push(text.to_string());
    }

    (text_parts, tool_uses)
}

pub(super) fn extract_single_tool_result(content: &Value) -> Option<(String, String)> {
    let items = content.as_array()?;
    if items.len() != 1 {
        return None;
    }
    let item = &items[0];
    if item.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    Some((
        item.get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        item.get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ))
}

pub(super) fn parse_tool_arguments(arguments: &Value) -> Result<Value> {
    match arguments {
        Value::String(raw) => serde_json::from_str(raw).map_err(Into::into),
        Value::Object(_) => Ok(arguments.clone()),
        Value::Null => Ok(json!({})),
        _ => Err(anyhow!("tool arguments must be a string or object")),
    }
}

use reqwest::StatusCode;

/// Retryable HTTP errors for Ollama, OpenAI-compatible backends, and web tools.
/// Official API backends (Codex/Gemini) handle their own retry.
pub(crate) fn is_retryable_http_error(error: &anyhow::Error) -> bool {
    if let Some(e) = error.downcast_ref::<reqwest::Error>() {
        if e.is_timeout() || e.is_connect() {
            return true;
        }
        if let Some(status) = e.status() {
            return status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS;
        }
    }
    let msg = error.to_string().to_lowercase();
    msg.contains("timeout")
        || msg.contains("timed out")
        || msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("unreachable")
        || (msg.contains("not found") && msg.contains("dns"))
}

/// Send a POST JSON request with exponential-backoff retry.
/// Checks response status inside the closure so 429/5xx trigger retry.
pub(crate) async fn retry_send_json(
    client: &reqwest::Client,
    url: &str,
    body: &Value,
    api_key: Option<&str>,
) -> Result<reqwest::Response> {
    (|| async {
        let mut request = client.post(url);
        if let Some(key) = api_key
            && !key.is_empty()
        {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let res = request.json(body).send().await.map_err(|e| anyhow!(e))?;
        let status = res.status();
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            let body_text = res.text().await.unwrap_or_default();
            let preview = body_text.chars().take(200).collect::<String>();
            return Err(anyhow!("HTTP {}: {preview}", status.as_u16()));
        }
        Ok(res)
    })
    .retry(ExponentialBuilder::default().with_jitter())
    .when(|e: &anyhow::Error| is_retryable_http_error(e))
    .await
}

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(300);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) fn http_client_for_target(base_url: &str) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .read_timeout(HTTP_READ_TIMEOUT);
    if should_bypass_proxy(base_url) {
        builder = builder.no_proxy();
    }
    builder.build().map_err(Into::into)
}

pub(super) async fn next_stream_item_with_idle_timeout<S, T>(
    stream: &mut S,
    label: &str,
) -> Result<Option<T>>
where
    S: Stream<Item = T> + Unpin,
{
    tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next())
        .await
        .map_err(|_| {
            anyhow!(
                "{label} stream produced no events for {} seconds",
                STREAM_IDLE_TIMEOUT.as_secs()
            )
        })
}

pub(super) fn should_bypass_proxy(base_url: &str) -> bool {
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };
    matches!(url.host_str(), Some("localhost") | Some("127.0.0.1"))
}

pub(super) fn context_budget_from_window(context_window_tokens: usize) -> ContextBudget {
    let reserved_output_tokens = (context_window_tokens / 8).clamp(1024, 16384);
    let compact_threshold_tokens = context_window_tokens
        .saturating_sub(reserved_output_tokens)
        .saturating_sub(2048);
    ContextBudget {
        context_window_tokens,
        reserved_output_tokens,
        compact_threshold_tokens,
    }
}

const DEEPSEEK_LONG_CONTEXT_WINDOW_TOKENS: usize = 1_048_576;
const OPENAI_LONG_CONTEXT_WINDOW_TOKENS: usize = 200_000;
const OPENAI_GPT4_CONTEXT_WINDOW_TOKENS: usize = 128_000;
const DEEPSEEK_LONG_CONTEXT_MODEL_MARKERS: &[&str] = &["deepseek-v4"];

pub(super) fn model_context_budget(model: &str) -> Option<ContextBudget> {
    let canonical = model.trim().to_ascii_lowercase();
    let context_window_tokens = if is_deepseek_long_context_model(&canonical) {
        DEEPSEEK_LONG_CONTEXT_WINDOW_TOKENS
    } else if canonical.contains("gpt-5")
        || canonical.contains("codex")
        || canonical.contains("gpt-4.1")
        || canonical.contains("gpt-4o")
    {
        OPENAI_LONG_CONTEXT_WINDOW_TOKENS
    } else if canonical.contains("gpt-4") {
        OPENAI_GPT4_CONTEXT_WINDOW_TOKENS
    } else {
        return None;
    };
    Some(context_budget_from_window(context_window_tokens))
}

fn is_deepseek_long_context_model(canonical_model: &str) -> bool {
    canonical_model.contains("deepseek")
        && DEEPSEEK_LONG_CONTEXT_MODEL_MARKERS
            .iter()
            .any(|marker| canonical_model.contains(marker))
}

pub(crate) fn hashed_embedding(text: &str, dim: usize) -> Vec<f32> {
    use sha2::{Digest, Sha256};

    let mut values = vec![0f32; dim];
    for token in text.split_whitespace() {
        let digest = Sha256::digest(token.as_bytes());
        let bucket = ((digest[0] as usize) << 8 | digest[1] as usize) % dim;
        let sign = if digest[2] % 2 == 0 { 1.0 } else { -1.0 };
        values[bucket] += sign;
    }

    let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut values {
            *value /= norm;
        }
    }
    values
}
