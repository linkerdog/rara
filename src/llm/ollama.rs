use std::collections::HashMap;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::StreamExt;
use rara_persistence::redaction::{redact_secrets, sanitize_url_for_display};
use serde_json::{Value, json};

use super::shared::{
    ContextBudget, LlmBackend, LlmStreamEvent, collect_assistant_content,
    context_budget_from_window, extract_single_tool_result, http_client_for_target,
    parse_tool_arguments, render_openai_message_content, retry_send_json,
};
use crate::agent::Message;
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

pub struct OllamaBackend {
    pub client: reqwest::Client,
    pub base_url: String,
    pub model: String,
    pub thinking: bool,
    pub num_ctx: Option<u32>,
}

impl OllamaBackend {
    pub fn new(
        base_url: String,
        model: String,
        thinking: bool,
        num_ctx: Option<u32>,
    ) -> Result<Self> {
        Ok(Self {
            client: http_client_for_target(&base_url)?,
            base_url,
            model,
            thinking,
            num_ctx,
        })
    }
}

#[async_trait]
impl LlmBackend for OllamaBackend {
    fn model_label(&self) -> Option<String> {
        Some(self.model.clone())
    }

    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        let endpoint = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let mut body = json!({
            "model": self.model,
            "messages": to_ollama_messages(messages),
            "stream": false,
            "think": self.thinking,
        });
        if let Some(options) = build_ollama_options(messages, tools, self.thinking, self.num_ctx) {
            body["options"] = options;
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": tool["name"],
                                "description": tool["description"],
                                "parameters": tool["input_schema"],
                            }
                        })
                    })
                    .collect(),
            );
        }

        let res = retry_send_json(&self.client, &endpoint, &body, None).await?;

        if !res.status().is_success() {
            return Err(anyhow!(
                "API Error at {}: {}",
                sanitize_url_for_display(&endpoint),
                redact_secrets(res.text().await?)
            ));
        }
        let resp_json: Value = res.json().await?;
        let message = &resp_json["message"];
        let mut content = Vec::new();
        if let Some(text) = message.get("content").and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            content.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for (idx, call) in tool_calls.iter().enumerate() {
                content.push(ContentBlock::ToolUse {
                    id: format!("ollama-tool-{}", idx + 1),
                    name: call["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    input: parse_tool_arguments(&call["function"]["arguments"])?,
                });
            }
        }

        Ok(LlmResponse {
            content,
            stop_reason: resp_json
                .get("done_reason")
                .and_then(Value::as_str)
                .map(str::to_string),
            usage: Some(TokenUsage {
                input_tokens: resp_json
                    .get("prompt_eval_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                output_tokens: resp_json
                    .get("eval_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                ..TokenUsage::default()
            }),
        })
    }

    async fn ask_streaming(
        &self,
        messages: &[Message],
        tools: &[Value],
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask_streaming_with_context(
            messages,
            tools,
            crate::llm::LlmTurnMetadata::default(),
            on_event,
        )
        .await
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: crate::llm::LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        let endpoint = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let mut body = json!({
            "model": self.model,
            "messages": to_ollama_messages(messages),
            "stream": true,
            "think": self.thinking,
        });
        if let Some(options) = build_ollama_options(messages, tools, self.thinking, self.num_ctx) {
            body["options"] = options;
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": tool["name"],
                                "description": tool["description"],
                                "parameters": tool["input_schema"],
                            }
                        })
                    })
                    .collect(),
            );
        }

        let res = retry_send_json(&self.client, &endpoint, &body, None).await?;
        if !res.status().is_success() {
            return Err(anyhow!(
                "API Error at {}: {}",
                sanitize_url_for_display(&endpoint),
                redact_secrets(res.text().await?)
            ));
        }

        let mut stream = res.bytes_stream();
        let mut buffer = Vec::new();
        let mut streamed_text = String::new();
        let mut streamed_tool_calls = Vec::new();
        let mut stop_reason = None;
        let mut input_tokens = 0u32;
        let mut output_tokens = 0u32;
        let mut saw_done = false;

        while let Some(chunk) = stream.next().await {
            metadata.ensure_not_cancelled()?;
            let chunk = chunk?;
            buffer.extend_from_slice(&chunk);

            while let Some(pos) = buffer.iter().position(|byte| *byte == b'\n') {
                let line = buffer.drain(..=pos).collect::<Vec<_>>();
                let payload = normalize_ollama_stream_line(&line);
                if payload.is_empty() {
                    continue;
                }
                let event: Value = serde_json::from_slice(&payload)?;
                saw_done |= apply_ollama_stream_event(
                    &event,
                    &mut streamed_text,
                    &mut streamed_tool_calls,
                    &mut stop_reason,
                    &mut input_tokens,
                    &mut output_tokens,
                    on_event,
                )?;
            }
        }

        let payload = normalize_ollama_stream_line(&buffer);
        if !payload.is_empty() {
            let event: Value = serde_json::from_slice(&payload)?;
            saw_done |= apply_ollama_stream_event(
                &event,
                &mut streamed_text,
                &mut streamed_tool_calls,
                &mut stop_reason,
                &mut input_tokens,
                &mut output_tokens,
                on_event,
            )?;
        }

        ensure_ollama_stream_completed(saw_done, &endpoint)?;

        let mut content = Vec::new();
        if !streamed_text.trim().is_empty() {
            content.push(ContentBlock::Text {
                text: streamed_text,
            });
        }
        for (idx, call) in streamed_tool_calls.iter().enumerate() {
            content.push(ContentBlock::ToolUse {
                id: format!("ollama-tool-{}", idx + 1),
                name: call.name.clone(),
                input: call.arguments.clone(),
            });
        }

        Ok(LlmResponse {
            content,
            stop_reason,
            usage: Some(TokenUsage {
                input_tokens,
                output_tokens,
                ..TokenUsage::default()
            }),
        })
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        let mut messages = messages.to_vec();
        messages.push(Message {
            role: "user".to_string(),
            content: json!(instruction),
        });
        let response = self.ask(&messages, &[]).await?;
        let text = response
            .content
            .into_iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(text)
    }

    fn context_budget(&self, messages: &[Message], tools: &[Value]) -> Option<ContextBudget> {
        let context_window_tokens = self.num_ctx.map(|value| value as usize).or_else(|| {
            suggest_ollama_num_ctx(messages, tools, self.thinking).map(|value| value as usize)
        })?;
        Some(context_budget_from_window(context_window_tokens))
    }
}

fn normalize_ollama_stream_line(line: &[u8]) -> Vec<u8> {
    line.iter()
        .copied()
        .filter(|byte| *byte != b'\n' && *byte != b'\r')
        .collect()
}

pub(super) fn apply_ollama_stream_event(
    event: &Value,
    streamed_text: &mut String,
    streamed_tool_calls: &mut Vec<OllamaToolCall>,
    stop_reason: &mut Option<String>,
    input_tokens: &mut u32,
    output_tokens: &mut u32,
    on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
) -> Result<bool> {
    if let Some(delta) = event
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        && !delta.is_empty()
    {
        on_event(LlmStreamEvent::TextDelta(delta.to_string()));
        streamed_text.push_str(delta);
    }

    if let Some(tool_calls) = event
        .get("message")
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        && !tool_calls.is_empty()
    {
        merge_ollama_stream_tool_calls(streamed_tool_calls, tool_calls);
    }

    if event.get("done").and_then(Value::as_bool).unwrap_or(false) {
        *stop_reason = event
            .get("done_reason")
            .and_then(Value::as_str)
            .map(str::to_string);
        *input_tokens = event
            .get("prompt_eval_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        *output_tokens = event.get("eval_count").and_then(Value::as_u64).unwrap_or(0) as u32;
        return Ok(true);
    }

    Ok(false)
}

pub(super) fn ensure_ollama_stream_completed(saw_done: bool, endpoint: &str) -> Result<()> {
    if saw_done {
        return Ok(());
    }
    Err(anyhow!(
        "Ollama stream at {} ended before the final done event",
        sanitize_url_for_display(endpoint)
    ))
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OllamaToolCall {
    pub name: String,
    pub arguments: Value,
}

fn merge_ollama_stream_tool_calls(
    streamed_tool_calls: &mut Vec<OllamaToolCall>,
    tool_calls: &[Value],
) {
    for tool_call in tool_calls {
        let Some(normalized) = normalize_ollama_tool_call(tool_call) else {
            continue;
        };
        if streamed_tool_calls
            .iter()
            .any(|existing| existing == &normalized)
        {
            continue;
        }
        streamed_tool_calls.push(normalized);
    }
}

fn normalize_ollama_tool_call(tool_call: &Value) -> Option<OllamaToolCall> {
    let function = tool_call.get("function")?;
    let name = function.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    let arguments = parse_tool_arguments(function.get("arguments").unwrap_or(&Value::Null)).ok()?;
    Some(OllamaToolCall {
        name: name.to_string(),
        arguments,
    })
}

pub(super) fn to_ollama_messages(messages: &[Message]) -> Vec<Value> {
    let mut ollama_messages = Vec::new();
    let mut tool_names_by_id = HashMap::new();
    for message in messages {
        match message.role.as_str() {
            "system" => ollama_messages.push(json!({
                "role": "system",
                "content": render_openai_message_content(&message.content),
            })),
            "assistant" => ollama_messages.push(render_ollama_assistant_message(
                &message.content,
                &mut tool_names_by_id,
            )),
            "user" => {
                if let Some(tool_result) =
                    extract_ollama_tool_result_message(&message.content, &tool_names_by_id)
                {
                    ollama_messages.push(tool_result);
                } else {
                    ollama_messages.push(json!({
                        "role": "user",
                        "content": render_openai_message_content(&message.content),
                    }));
                }
            }
            other => ollama_messages.push(json!({
                "role": other,
                "content": render_openai_message_content(&message.content),
            })),
        }
    }
    ollama_messages
}

pub(super) fn build_ollama_options(
    messages: &[Message],
    tools: &[Value],
    thinking: bool,
    configured_num_ctx: Option<u32>,
) -> Option<Value> {
    let num_ctx =
        configured_num_ctx.or_else(|| suggest_ollama_num_ctx(messages, tools, thinking))?;
    Some(json!({
        "num_ctx": num_ctx,
    }))
}

pub(super) fn suggest_ollama_num_ctx(
    messages: &[Message],
    tools: &[Value],
    thinking: bool,
) -> Option<u32> {
    if messages.is_empty() {
        return None;
    }

    let mut tool_results = 0usize;
    let mut assistant_tool_uses = 0usize;
    let mut combined_text = String::new();

    for message in messages {
        if !combined_text.is_empty() {
            combined_text.push('\n');
        }
        combined_text.push_str(&render_openai_message_content(&message.content));

        if let Some(items) = message.content.as_array() {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("tool_result") => tool_results += 1,
                    Some("tool_use") => assistant_tool_uses += 1,
                    _ => {}
                }
            }
        }
    }

    let has_plan_markers = combined_text.contains("<proposed_plan>")
        || combined_text.contains("<plan>")
        || combined_text.contains("Plan Mode");
    let has_runtime_markers = combined_text.contains("<agent_runtime>");

    let lower_text = combined_text.to_ascii_lowercase();
    let review_like = [
        "architecture",
        "codebase",
        "repository",
        "repo",
        "review",
        "inspect",
        "analyze",
        "improve",
        "directory",
        "project structure",
    ]
    .iter()
    .any(|needle| lower_text.contains(needle));

    if has_plan_markers || has_runtime_markers || tool_results >= 2 || assistant_tool_uses >= 2 {
        return Some(32768);
    }

    if review_like || messages.len() >= 10 || (!tools.is_empty() && thinking) {
        return Some(24576);
    }

    if !tools.is_empty() || thinking {
        return Some(16384);
    }

    None
}

fn render_ollama_assistant_message(
    content: &Value,
    tool_names_by_id: &mut HashMap<String, String>,
) -> Value {
    let (text_parts, assistant_tool_uses) = collect_assistant_content(content);
    let mut tool_calls = Vec::new();
    for tool_use in assistant_tool_uses {
        if !tool_use.id.is_empty() {
            tool_names_by_id.insert(tool_use.id.clone(), tool_use.name.clone());
        }
        tool_calls.push(json!({
            "function": {
                "name": tool_use.name,
                "arguments": tool_use.input,
            }
        }));
    }

    let mut message = json!({
        "role": "assistant",
        "content": text_parts.join("\n\n"),
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    message
}

fn extract_ollama_tool_result_message(
    content: &Value,
    tool_names_by_id: &HashMap<String, String>,
) -> Option<Value> {
    let (tool_use_id, tool_content) = extract_single_tool_result(content)?;
    Some(json!({
        "role": "tool",
        "tool_name": tool_names_by_id.get(tool_use_id.as_str()).cloned().unwrap_or_default(),
        "content": tool_content,
    }))
}
