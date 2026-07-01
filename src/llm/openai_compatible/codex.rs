use anyhow::{Result, anyhow};
use async_trait::async_trait;
use backon::{ExponentialBuilder, Retryable};
use codex_login::default_client::default_headers as codex_default_headers;
use eventsource_stream::Eventsource;
use rara_persistence::redaction::sanitize_url_for_display;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};

use super::super::codex_tools_compat::ToolDefinition;
use super::super::codex_tools_compat::ToolSpec;
use super::super::codex_tools_compat::create_tools_json_for_responses_api;
use super::super::codex_tools_compat::parse_tool_input_schema;
use super::super::codex_tools_compat::tool_definition_to_responses_api_tool;
use super::super::shared::{
    ContextBudget, LlmBackend, LlmStreamEvent, collect_assistant_content, is_retryable_http_error,
    model_context_budget, next_stream_item_with_idle_timeout, parse_tool_arguments,
    render_openai_message_content,
};
use super::{
    CodexBackend, OpenAiApiError, OpenAiCompatibleBackend, api_error_from_response,
    is_auxiliary_model_retryable_error,
};
use crate::agent::Message;
use crate::llm::{ContentBlock, LlmResponse};

impl CodexBackend {
    pub fn new(
        api_key: Option<SecretString>,
        base_url: String,
        model: String,
        reasoning_effort: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            client: super::http_client_for_target(&base_url)?,
            api_key,
            base_url,
            model,
            auxiliary_model: None,
            reasoning_effort,
        })
    }

    pub fn with_auxiliary_model(mut self, auxiliary_model: Option<String>) -> Self {
        self.auxiliary_model = auxiliary_model
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty());
        self
    }

    fn summary_model(&self) -> &str {
        self.auxiliary_model
            .as_deref()
            .unwrap_or(self.model.as_str())
    }

    fn endpoint_url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        format!("{base}/{path}")
    }

    async fn ask_responses_streaming(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[Value],
        metadata: crate::llm::LlmTurnMetadata,
        mut on_event: Option<&mut (dyn FnMut(LlmStreamEvent) + Send)>,
    ) -> Result<LlmResponse> {
        let body = build_codex_responses_request(
            model,
            messages,
            tools,
            self.reasoning_effort.as_deref(),
        )?;
        let responses_url = self.endpoint_url("responses");
        let api_key = self.api_key.as_ref().map(|k| k.expose_secret());
        let res = (|| async {
            let mut request = self.client.post(&responses_url);
            for (name, value) in &codex_default_headers() {
                request = request.header(name, value);
            }
            if let Some(key) = api_key
                && !key.is_empty()
            {
                request = request.header("Authorization", format!("Bearer {key}"));
            }
            request.json(&body).send().await.map_err(|e| anyhow!(e))
        })
        .retry(ExponentialBuilder::default().with_jitter())
        .when(|e: &anyhow::Error| is_retryable_http_error(e))
        .await?;
        if !res.status().is_success() {
            return Err(
                anyhow::Error::new(api_error_from_response(res).await).context(format!(
                    "API Error at {}",
                    sanitize_url_for_display(&responses_url)
                )),
            );
        }

        let mut stream = res.bytes_stream().eventsource();
        let mut output_items = Vec::new();
        let mut usage = None;
        let mut completed = false;
        let mut streamed_text = String::new();

        while let Some(event) = next_stream_item_with_idle_timeout(&mut stream, "Codex SSE").await?
        {
            metadata.ensure_not_cancelled()?;
            let event =
                event.map_err(|error| anyhow!("Failed to decode Codex SSE event: {error}"))?;
            if event.data.trim().is_empty() {
                continue;
            }
            let payload: Value = serde_json::from_str(&event.data)
                .map_err(|error| anyhow!("Failed to parse Codex SSE payload: {error}"))?;
            completed |= apply_codex_stream_event(
                &payload,
                &mut output_items,
                &mut usage,
                &mut streamed_text,
                &mut on_event,
            )?;
        }

        if !completed {
            return Err(anyhow!(
                "Codex response stream ended before response.completed"
            ));
        }

        parse_codex_response(&build_codex_stream_response(
            output_items,
            usage,
            streamed_text,
            "completed",
        ))
    }
}

#[async_trait]
impl LlmBackend for CodexBackend {
    fn model_label(&self) -> Option<String> {
        Some(self.model.clone())
    }

    async fn ask(&self, m: &[Message], t: &[Value]) -> Result<LlmResponse> {
        self.ask_responses_streaming(
            self.model.as_str(),
            m,
            t,
            crate::llm::LlmTurnMetadata::default(),
            None,
        )
        .await
    }

    async fn ask_streaming(
        &self,
        messages: &[Message],
        tools: &[Value],
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask_responses_streaming(
            self.model.as_str(),
            messages,
            tools,
            crate::llm::LlmTurnMetadata::default(),
            Some(on_event),
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
        self.ask_responses_streaming(
            self.model.as_str(),
            messages,
            tools,
            metadata,
            Some(on_event),
        )
        .await
    }

    async fn embed(&self, t: &str) -> Result<Vec<f32>> {
        OpenAiCompatibleBackend::new(
            self.api_key.clone(),
            self.base_url.clone(),
            self.model.clone(),
        )?
        .embed(t)
        .await
    }

    async fn summarize(&self, m: &[Message], instruction: &str) -> Result<String> {
        let mut messages = m.to_vec();
        messages.push(Message {
            role: "user".to_string(),
            content: json!(instruction),
        });
        let summary_model = self.summary_model();
        let response = self
            .ask_responses_streaming(
                summary_model,
                &messages,
                &[],
                crate::llm::LlmTurnMetadata::default(),
                None,
            )
            .await;
        let response = if summary_model != self.model.as_str()
            && response
                .as_ref()
                .is_err_and(is_auxiliary_model_retryable_error)
        {
            self.ask_responses_streaming(
                self.model.as_str(),
                &messages,
                &[],
                crate::llm::LlmTurnMetadata::default(),
                None,
            )
            .await?
        } else {
            response?
        };
        Ok(response
            .content
            .into_iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } if !text.trim().is_empty() => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n"))
    }

    fn context_budget(&self, messages: &[Message], tools: &[Value]) -> Option<ContextBudget> {
        let main_budget = model_context_budget(self.model.as_str()).or_else(|| {
            let inner = OpenAiCompatibleBackend::new(
                self.api_key.clone(),
                self.base_url.clone(),
                self.model.clone(),
            )
            .ok()?;
            inner.context_budget(messages, tools)
        });
        let summary_model = self.summary_model();
        let summary_budget = if summary_model == self.model.as_str() {
            main_budget
        } else {
            model_context_budget(summary_model).or(main_budget)
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
}

pub(crate) fn build_codex_responses_request(
    model: &str,
    messages: &[Message],
    tools: &[Value],
    reasoning_effort: Option<&str>,
) -> Result<Value> {
    let mut body = json!({
        "model": model,
        "input": to_codex_input_items(messages),
        "tools": to_codex_tools(tools)?,
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
        "include": [],
        "instructions": build_codex_instructions(messages),
        "text": Value::Null,
        "client_metadata": Value::Null,
    });
    if let Some(reasoning_effort) = reasoning_effort
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body["reasoning"] = json!({ "effort": reasoning_effort });
    }
    Ok(body)
}

pub(crate) fn build_codex_stream_response(
    mut output: Vec<Value>,
    usage: Option<Value>,
    streamed_text: String,
    status: &str,
) -> Value {
    if !streamed_text.trim().is_empty() && !output_has_text_message(&output) {
        output.insert(
            0,
            json!({
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": streamed_text,
                }]
            }),
        );
    }
    let mut response = serde_json::Map::new();
    response.insert("status".to_string(), Value::String(status.to_string()));
    response.insert("output".to_string(), Value::Array(output));
    if let Some(usage) = usage {
        response.insert("usage".to_string(), usage);
    }
    Value::Object(response)
}

fn output_has_text_message(output: &[Value]) -> bool {
    output.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("message")
            && extract_codex_output_text(item.get("content"))
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false)
    })
}

fn codex_output_item_identity(item: &Value) -> Option<&str> {
    item.get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
}

fn upsert_codex_output_item(output_items: &mut Vec<Value>, item: &Value) {
    let Some(identity) = codex_output_item_identity(item) else {
        output_items.push(item.clone());
        return;
    };

    if let Some(existing) = output_items
        .iter_mut()
        .find(|existing| codex_output_item_identity(existing) == Some(identity))
    {
        *existing = item.clone();
    } else {
        output_items.push(item.clone());
    }
}

pub(crate) fn apply_codex_stream_event(
    payload: &Value,
    output_items: &mut Vec<Value>,
    usage: &mut Option<Value>,
    streamed_text: &mut String,
    on_event: &mut Option<&mut (dyn FnMut(LlmStreamEvent) + Send)>,
) -> Result<bool> {
    match payload.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => {
            if let Some(delta) = payload.get("delta").and_then(Value::as_str) {
                streamed_text.push_str(delta);
                if let Some(callback) = on_event.as_mut() {
                    callback(LlmStreamEvent::TextDelta(delta.to_string()));
                }
            }
            Ok(false)
        }
        Some("response.reasoning_summary_text.delta") | Some("response.reasoning_text.delta") => {
            if let Some(delta) = payload.get("delta").and_then(Value::as_str)
                && let Some(callback) = on_event.as_mut()
            {
                callback(LlmStreamEvent::ReasoningDelta(delta.to_string()));
            }
            Ok(false)
        }
        Some("response.output_item.added")
        | Some("response.output_item.done")
        | Some("conversation.item.done") => {
            if let Some(item) = payload.get("item") {
                upsert_codex_output_item(output_items, item);
            }
            Ok(false)
        }
        Some("response.done") | Some("response.completed") => {
            *usage = payload.get("response").and_then(|response| {
                let usage = response.get("usage")?;
                (!usage.is_null()).then(|| usage.clone())
            });
            Ok(true)
        }
        Some("response.failed") => {
            let Some(error) = payload
                .get("response")
                .and_then(|response| response.get("error"))
            else {
                return Err(anyhow!("response.failed event received"));
            };
            Err(anyhow::Error::new(
                OpenAiApiError::from_response_failed_error(error),
            ))
        }
        Some("response.incomplete") => Err(anyhow!(
            "Incomplete response returned, reason: {}",
            payload
                .get("response")
                .and_then(|response| response.get("incomplete_details"))
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        )),
        _ => Ok(false),
    }
}

fn to_codex_tools(tools: &[Value]) -> Result<Vec<Value>> {
    let mut tool_specs = Vec::with_capacity(tools.len());
    for tool in tools {
        let tool_name = tool["name"]
            .as_str()
            .ok_or_else(|| anyhow!("Codex tool is missing a 'name' field"))?;
        let input_schema = parse_tool_input_schema(&tool["input_schema"]).map_err(|error| {
            anyhow!("Failed to parse Codex tool schema for '{tool_name}': {error}")
        })?;
        let responses_tool = tool_definition_to_responses_api_tool(ToolDefinition {
            name: tool_name.to_string(),
            description: tool["description"].as_str().unwrap_or_default().to_string(),
            input_schema,
            output_schema: None,
            defer_loading: false,
        });
        tool_specs.push(ToolSpec::Function(responses_tool));
    }
    let mut tools_json = create_tools_json_for_responses_api(&tool_specs)
        .map_err(|error| anyhow!("Failed to serialize Codex tools for Responses API: {error}"))?;
    for tool in &mut tools_json {
        normalize_chatgpt_codex_function_schema(tool);
    }
    Ok(tools_json)
}

fn normalize_chatgpt_codex_function_schema(tool: &mut Value) {
    let Some(parameters) = tool.get_mut("parameters").and_then(Value::as_object_mut) else {
        return;
    };
    parameters.remove("anyOf");
    parameters.remove("oneOf");
    parameters.remove("allOf");
    parameters.remove("not");
    parameters.remove("enum");
    parameters.insert("type".to_string(), Value::String("object".to_string()));
    parameters
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
}

pub(crate) fn to_codex_input_items(messages: &[Message]) -> Vec<Value> {
    let mut items = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "assistant" => items.extend(render_codex_assistant_items(&message.content)),
            "user" => items.extend(render_codex_user_items(&message.content)),
            "system" => {}
            role => items.push(render_codex_message(role, &message.content, false)),
        }
    }
    items
}

fn build_codex_instructions(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|message| message.role == "system")
        .map(|message| render_openai_message_content(&message.content))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_codex_message(role: &str, content: &Value, assistant_output: bool) -> Value {
    let text = render_openai_message_content(content);
    let content_item_type = if assistant_output {
        "output_text"
    } else {
        "input_text"
    };
    json!({
        "type": "message",
        "role": role,
        "content": [{
            "type": content_item_type,
            "text": text,
        }],
    })
}

fn render_codex_assistant_items(content: &Value) -> Vec<Value> {
    let (text_parts, assistant_tool_uses) = collect_assistant_content(content);
    let mut items = Vec::new();
    if !text_parts.is_empty() {
        items.push(render_codex_message(
            "assistant",
            &Value::String(text_parts.join("\n\n")),
            true,
        ));
    }
    for tool_use in assistant_tool_uses {
        items.push(json!({
            "type": "function_call",
            "name": tool_use.name,
            "arguments": serde_json::to_string(&tool_use.input)
                .unwrap_or_else(|_| "{}".to_string()),
            "call_id": tool_use.id,
        }));
    }
    items
}

fn render_codex_user_items(content: &Value) -> Vec<Value> {
    let Some(raw_items) = content.as_array() else {
        return vec![render_codex_message("user", content, false)];
    };

    let mut rendered = Vec::new();
    let mut text_parts = Vec::new();
    let flush_text = |rendered: &mut Vec<Value>, text_parts: &mut Vec<String>| {
        if !text_parts.is_empty() {
            rendered.push(render_codex_message(
                "user",
                &Value::String(text_parts.join("\n\n")),
                false,
            ));
            text_parts.clear();
        }
    };

    for item in raw_items {
        match item.get("type").and_then(Value::as_str) {
            Some("tool_result") => {
                flush_text(&mut rendered, &mut text_parts);
                if let Some(tool_result) = extract_codex_tool_result_item(item) {
                    rendered.push(tool_result);
                }
            }
            Some("text") => {
                if let Some(text) = item.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    text_parts.push(text.to_string());
                }
            }
            _ => {}
        }
    }
    flush_text(&mut rendered, &mut text_parts);

    if rendered.is_empty() {
        vec![render_codex_message("user", content, false)]
    } else {
        rendered
    }
}

fn extract_codex_tool_result_item(content: &Value) -> Option<Value> {
    let tool_use_id = content
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tool_content = content
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if tool_use_id.is_empty() {
        return None;
    }
    Some(json!({
        "type": "function_call_output",
        "call_id": tool_use_id,
        "output": tool_content,
    }))
}

pub(crate) fn parse_codex_response(resp_json: &Value) -> Result<LlmResponse> {
    let mut content = Vec::new();
    if let Some(items) = resp_json.get("output").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(text) = extract_codex_output_text(item.get("content"))
                        && !text.trim().is_empty()
                    {
                        content.push(ContentBlock::Text { text });
                    }
                }
                Some("function_call") => {
                    let id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let input =
                        parse_tool_arguments(item.get("arguments").unwrap_or(&Value::Null))?;
                    content.push(ContentBlock::ToolUse { id, name, input });
                }
                _ => {}
            }
        }
    }
    let usage = resp_json.get("usage").map(super::parse_openai_token_usage);
    Ok(LlmResponse {
        content,
        stop_reason: resp_json
            .get("status")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
        usage,
    })
}

fn extract_codex_output_text(content: Option<&Value>) -> Option<String> {
    let items = content?.as_array()?;
    let texts = items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("output_text") => item.get("text").and_then(Value::as_str).map(str::to_string),
            _ => None,
        })
        .collect::<Vec<_>>();
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n\n"))
    }
}
