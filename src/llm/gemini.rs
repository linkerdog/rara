use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use backon::{ExponentialBuilder, Retryable};
use eventsource_stream::Eventsource;
use rara_persistence::redaction::sanitize_url_for_display;
use serde_json::{Value, json};
use uuid::Uuid;

use super::gemini_schema::sanitize_gemini_schema;
use super::shared::{
    ContextBudget, LlmBackend, LlmStreamEvent, LlmTurnMetadata, context_budget_from_window,
    is_retryable_http_error, next_stream_item_with_idle_timeout,
};
use crate::agent::Message;
use crate::google_oauth::GoogleOAuthManager;
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

/// Code Assist production endpoint.
const CODE_ASSIST_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";

/// Auth mode for Gemini.
#[derive(Debug, Clone)]
pub enum GeminiAuthMode {
    /// Google Code Assist — OAuth access token.
    OAuth { oauth: GoogleOAuthManager },
}

/// Gemini backend supporting both AI Studio (API key) and Code Assist
/// (OAuth) paths with native Gemini protocol.
#[derive(Clone)]
pub struct GeminiBackend {
    auth: GeminiAuthMode,
    model: String,
    client: reqwest::Client,
}

impl GeminiBackend {
    pub fn with_oauth(oauth: GoogleOAuthManager, model: String) -> Result<Self> {
        Ok(Self {
            auth: GeminiAuthMode::OAuth { oauth },
            model,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()?,
        })
    }

    fn base_endpoint(&self) -> &str {
        CODE_ASSIST_ENDPOINT
    }

    async fn send_gemini_request(&self, body: &Value, stream: bool) -> Result<reqwest::Response> {
        let endpoint = self.base_endpoint();
        let model = &self.model;

        let cred = self.code_assist_credential().await?;
        let project_id = cred.project_id.as_deref().unwrap_or("unknown");
        let url = format!(
            "{}/v1internal/projects/-/locations/global/models/{}:{}",
            endpoint,
            model,
            if stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            }
        );
        let envelope = json!({
            "project": project_id,
            "model": model,
            "user_prompt_id": Uuid::new_v4().to_string(),
            "request": body,
        });
        let request_builder = self
            .client
            .post(&url)
            .bearer_auth(cred.access_token)
            .json(&envelope);

        let res = (|| async {
            request_builder
                .try_clone()
                .ok_or_else(|| anyhow!("Request not cloneable"))?
                .send()
                .await
                .map_err(|e| anyhow!(e))
        })
        .retry(ExponentialBuilder::default().with_jitter())
        .when(|e: &anyhow::Error| is_retryable_http_error(e))
        .await
        .map_err(|e| {
            anyhow!(
                "Gemini API request failed at {}: {}",
                sanitize_url_for_display(&url),
                e
            )
        })?;
        Ok(res)
    }

    async fn code_assist_credential(&self) -> Result<crate::google_oauth::GoogleCredential> {
        let GeminiAuthMode::OAuth { oauth } = &self.auth;
        oauth.load_credential().await
    }
}

#[async_trait]
impl LlmBackend for GeminiBackend {
    fn model_label(&self) -> Option<String> {
        Some(self.model.clone())
    }

    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        let body = build_gemini_request(messages, tools)?;
        let res = self.send_gemini_request(&body, false).await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            bail!("Gemini API error ({}): {}", status, body);
        }

        let resp: Value = res.json().await?;
        parse_gemini_response(&resp)
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        let body = build_gemini_request(messages, tools)?;
        let res = self.send_gemini_request(&body, true).await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            bail!("Gemini streaming API error ({}): {}", status, body);
        }

        let mut stream = res.bytes_stream().eventsource();
        let mut streamed_text = String::new();
        let mut accumulated_response: Option<Value> = None;

        while let Some(event) =
            next_stream_item_with_idle_timeout(&mut stream, "Gemini SSE").await?
        {
            metadata.ensure_not_cancelled()?;
            let event = event.map_err(|e| anyhow!("SSE error: {e}"))?;
            let data = event.data.trim();
            if data.is_empty() {
                continue;
            }

            // Gemini streaming returns JSON arrays of response objects.
            let chunk: Value =
                serde_json::from_str(data).context("Failed to parse Gemini SSE chunk")?;

            // Accumulate the response and extract text deltas.
            if let Some(candidates) = chunk.as_array() {
                for candidate in candidates {
                    if let Some(content) = candidate
                        .get("content")
                        .and_then(|c| c.get("parts"))
                        .and_then(|p| p.as_array())
                    {
                        for part in content {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str())
                                && !text.is_empty()
                            {
                                streamed_text.push_str(text);
                                on_event(LlmStreamEvent::TextDelta(text.to_string()));
                            }
                        }
                    }
                    // Track the most complete candidate as the response.
                    if candidate.get("finishReason").is_some() {
                        accumulated_response = Some(candidate.clone());
                    }
                }
            } else {
                // Single response object.
                if chunk.get("candidates").is_some() {
                    accumulated_response = Some(chunk);
                }
            }
        }

        // If we have a final accumulated response, parse it fully;
        // otherwise build from streamed text.
        if let Some(resp) = accumulated_response {
            let mut response = parse_gemini_candidate(&resp)?;
            // Replace text with streamed version for accuracy.
            if !streamed_text.is_empty()
                && response
                    .content
                    .iter()
                    .all(|b| !matches!(b, ContentBlock::Text { .. }))
            {
                response.content.push(ContentBlock::Text {
                    text: streamed_text,
                });
            }
            Ok(response)
        } else if !streamed_text.is_empty() {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: streamed_text,
                }],
                stop_reason: Some("STOP".to_string()),
                usage: None,
            })
        } else {
            Err(anyhow!("Gemini streaming produced no content"))
        }
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        // Use a flash model for summarization.
        let summary_messages = {
            let mut msgs = messages.to_vec();
            msgs.insert(
                0,
                Message {
                    role: "user".to_string(),
                    content: json!(instruction),
                },
            );
            msgs
        };

        let response = self.ask(&summary_messages, &[]).await?;
        for block in &response.content {
            if let ContentBlock::Text { text } = block {
                return Ok(text.clone());
            }
        }
        Ok(String::new())
    }

    fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
        gemini_context_budget(&self.model)
    }
}

// ── Request building ──────────────────────────────────────────────

fn build_gemini_request(messages: &[Message], tools: &[Value]) -> Result<Value> {
    let mut contents = Vec::new();
    let mut system_instruction: Option<Value> = None;

    for msg in messages {
        let role = msg.role.as_str();
        match role {
            "system" => {
                let text = extract_text_content(&msg.content);
                system_instruction = Some(json!({
                    "parts": [{"text": text}]
                }));
            }
            "user" => {
                let text = extract_text_content(&msg.content);
                contents.push(json!({
                    "role": "user",
                    "parts": [{"text": text}]
                }));
            }
            "assistant" => {
                let parts = convert_assistant_parts(&msg.content);
                contents.push(json!({
                    "role": "model",
                    "parts": parts
                }));
            }
            "tool" => {
                // Tool result — find the function name and response.
                let (tool_name, tool_result) = extract_tool_result(&msg.content);
                contents.push(json!({
                    "role": "function",
                    "parts": [{
                        "functionResponse": {
                            "name": tool_name,
                            "response": tool_result
                        }
                    }]
                }));
            }
            _ => {
                let text = extract_text_content(&msg.content);
                if !text.is_empty() {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{"text": text}]
                    }));
                }
            }
        }
    }

    let mut request = json!({
        "contents": contents,
    });

    if let Some(sys) = system_instruction {
        request["systemInstruction"] = sys;
    }

    if !tools.is_empty() {
        let function_declarations: Vec<Value> = tools
            .iter()
            .map(|tool| {
                let mut decl = json!({
                    "name": tool["name"],
                    "description": tool.get("description").unwrap_or(&json!("")),
                });
                let schema = tool
                    .get("input_schema")
                    .map(sanitize_gemini_schema)
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
                decl["parameters"] = schema;
                decl
            })
            .collect();

        request["tools"] = json!([{
            "functionDeclarations": function_declarations
        }]);
    }

    Ok(request)
}

fn extract_text_content(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let texts: Vec<String> = arr
            .iter()
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .map(|s| s.to_string())
            .collect();
        return texts.join("\n");
    }
    String::new()
}

fn convert_assistant_parts(content: &Value) -> Vec<Value> {
    let mut parts = Vec::new();

    if let Some(arr) = content.as_array() {
        for block in arr {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str())
                        && !text.is_empty()
                    {
                        parts.push(json!({"text": text}));
                    }
                }
                Some("tool_use") => {
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let input = block.get("input").cloned().unwrap_or(json!({}));
                    parts.push(json!({
                        "functionCall": {
                            "name": name,
                            "args": input
                        }
                    }));
                }
                _ => {}
            }
        }
    } else if let Some(s) = content.as_str()
        && !s.is_empty()
    {
        parts.push(json!({"text": s}));
    }

    if parts.is_empty() {
        parts.push(json!({"text": ""}));
    }
    parts
}

fn extract_tool_result(content: &Value) -> (String, Value) {
    // Look for tool_use result in the content array.
    if let Some(arr) = content.as_array() {
        for block in arr {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                let name = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let result = block.get("content").cloned().unwrap_or(json!(""));
                return (name, result);
            }
        }
    }

    // Fallback: treat the whole content as a plain response.
    (
        "unknown".to_string(),
        json!({"content": content.to_string()}),
    )
}

// ── Response parsing ──────────────────────────────────────────────

fn parse_gemini_response(resp: &Value) -> Result<LlmResponse> {
    let candidates = resp
        .get("candidates")
        .and_then(|c| c.as_array())
        .context("No candidates in Gemini response")?;

    if candidates.is_empty() {
        bail!("Empty candidates in Gemini response");
    }

    parse_gemini_candidate(&candidates[0])
}

fn parse_gemini_candidate(candidate: &Value) -> Result<LlmResponse> {
    let mut content_blocks = Vec::new();
    let finish_reason = candidate
        .get("finishReason")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string());

    if let Some(parts) = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(|t| t.as_str())
                && !text.is_empty()
            {
                content_blocks.push(ContentBlock::Text {
                    text: text.to_string(),
                });
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = fc.get("args").cloned().unwrap_or(json!({}));
                let id = format!("call-{}", Uuid::new_v4());
                content_blocks.push(ContentBlock::ToolUse {
                    id,
                    name,
                    input: args,
                });
            }
        }
    }

    let usage = parse_gemini_usage(candidate);

    Ok(LlmResponse {
        content: content_blocks,
        stop_reason: finish_reason,
        usage,
    })
}

fn parse_gemini_usage(candidate: &Value) -> Option<TokenUsage> {
    let usage = candidate.get("usageMetadata")?;
    let input_tokens = usage
        .get("promptTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let output_tokens = usage
        .get("candidatesTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Some(TokenUsage {
        input_tokens,
        output_tokens,
        cache_hit_tokens: 0,
        cache_miss_tokens: 0,
    })
}

// ── Context budget ────────────────────────────────────────────────

const GEMINI_LONG_CONTEXT_WINDOW: usize = 1_048_576;
const GEMINI_MEDIUM_CONTEXT_WINDOW: usize = 128_000;

/// Models with 1M+ token context window.
const GEMINI_LONG_CONTEXT_MODELS: &[&str] = &["gemini-2.5-pro", "gemini-2.5-flash", "gemini-3"];

fn gemini_context_budget(model: &str) -> Option<ContextBudget> {
    let canonical = model.trim().to_ascii_lowercase();
    let window = if GEMINI_LONG_CONTEXT_MODELS
        .iter()
        .any(|m| canonical.contains(m))
    {
        GEMINI_LONG_CONTEXT_WINDOW
    } else if canonical.contains("gemini") {
        GEMINI_MEDIUM_CONTEXT_WINDOW
    } else {
        return None;
    };
    Some(context_budget_from_window(window))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_simple_request() {
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: json!("You are helpful."),
            },
            Message {
                role: "user".to_string(),
                content: json!("Hello"),
            },
        ];
        let req = build_gemini_request(&messages, &[]).unwrap();
        assert_eq!(
            req["systemInstruction"]["parts"][0]["text"],
            "You are helpful."
        );
        assert_eq!(req["contents"][0]["role"], "user");
        assert_eq!(req["contents"][0]["parts"][0]["text"], "Hello");
    }

    #[test]
    fn sanitizes_tool_schemas() {
        let tools = vec![json!({
            "name": "search",
            "description": "Search the web",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "additionalProperties": false}
                },
                "additionalProperties": false
            }
        })];
        let req = build_gemini_request(&[], &tools).unwrap();
        let decls = &req["tools"][0]["functionDeclarations"][0];
        let params = &decls["parameters"];
        assert!(params.get("additionalProperties").is_none());
        let query = &params["properties"]["query"];
        assert!(query.get("additionalProperties").is_none());
    }

    #[test]
    fn parses_text_response() {
        let resp = json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello!"}],
                    "role": "model"
                },
                "finishReason": "STOP"
            }]
        });
        let result = parse_gemini_response(&resp).unwrap();
        assert_eq!(result.content.len(), 1);
        if let ContentBlock::Text { text } = &result.content[0] {
            assert_eq!(text, "Hello!");
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn parses_tool_call() {
        let resp = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "search",
                            "args": {"query": "rust"}
                        }
                    }],
                    "role": "model"
                },
                "finishReason": "STOP"
            }]
        });
        let result = parse_gemini_response(&resp).unwrap();
        assert_eq!(result.content.len(), 1);
        if let ContentBlock::ToolUse { name, input, .. } = &result.content[0] {
            assert_eq!(name, "search");
            assert_eq!(input["query"], "rust");
        } else {
            panic!("expected tool_use block");
        }
    }

    #[test]
    fn parses_token_usage() {
        let resp = json!({
            "candidates": [{
                "content": {"parts": [{"text": "ok"}], "role": "model"},
                "finishReason": "STOP",
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 2,
                    "totalTokenCount": 12
                }
            }]
        });
        let result = parse_gemini_response(&resp).unwrap();
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 2);
    }
}
