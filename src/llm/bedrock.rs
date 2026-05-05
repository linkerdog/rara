// AWS Bedrock backend using the Converse API.
//
// Bedrock Converse provides a unified messages API across Bedrock models
// (Claude, Llama, Nova, etc.) similar to the OpenAI chat completions API.
// Tool use / function calling is supported natively.

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::types::{
    ContentBlock as BedrockContentBlock, ConversationRole, InferenceConfiguration,
    Message as BedrockMessage, StopReason, Tool, ToolConfiguration, ToolInputSchema,
    ToolResultBlock, ToolResultContentBlock, ToolResultStatus, ToolSpecification, ToolUseBlock,
};
use aws_smithy_types::{Document, Number};
use serde_json::{Value, json};

use super::shared::{ContextBudget, LlmBackend, LlmStreamEvent, LlmTurnMetadata};
use crate::agent::Message;
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

const DEFAULT_CONTEXT_WINDOW: usize = 200_000;
const DEFAULT_OUTPUT_TOKENS: i32 = 8_192;

fn model_context_window(model_id: &str) -> Option<(usize, usize)> {
    let lower = model_id.to_lowercase();
    if lower.contains("claude-sonnet-4") || lower.contains("claude-3-5-sonnet") {
        Some((200_000, 8_192))
    } else if lower.contains("claude-3-opus") {
        Some((200_000, 4_096))
    } else if lower.contains("claude-3-haiku") {
        Some((200_000, 4_096))
    } else if lower.contains("claude-3") {
        Some((200_000, 4_096))
    } else if lower.contains("claude") {
        Some((200_000, 4_096))
    } else if lower.contains("llama") {
        Some((128_000, 2_048))
    } else if lower.contains("nova-pro") {
        Some((300_000, 5_000))
    } else if lower.contains("nova-lite") {
        Some((300_000, 5_000))
    } else if lower.contains("nova") {
        Some((300_000, 5_000))
    } else if lower.contains("command") {
        Some((128_000, 4_096))
    } else if lower.contains("mistral") {
        Some((128_000, 4_096))
    } else {
        None
    }
}

pub struct BedrockBackend {
    client: BedrockClient,
    model_id: String,
    region: String,
}

impl BedrockBackend {
    pub async fn new(region: Option<String>, model_id: String) -> Result<Self> {
        let sdk_config = aws_config::load_from_env().await;
        let client = BedrockClient::new(&sdk_config);

        Ok(Self {
            client,
            model_id,
            region: region.unwrap_or_else(|| "default".to_string()),
        })
    }
}

// ── serde_json::Value ↔ aws_smithy_types::Document ──

fn value_to_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Document::Number(Number::NegInt(i))
            } else if let Some(u) = n.as_u64() {
                Document::Number(Number::PosInt(u))
            } else if let Some(f) = n.as_f64() {
                Document::Number(Number::Float(f))
            } else {
                Document::Null
            }
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(arr) => Document::Array(arr.iter().map(value_to_document).collect()),
        Value::Object(obj) => {
            let map: std::collections::HashMap<String, Document> = obj
                .iter()
                .map(|(k, v)| (k.clone(), value_to_document(v)))
                .collect();
            Document::Object(map)
        }
    }
}

fn document_to_value(doc: &Document) -> Value {
    match doc {
        Document::Object(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                obj.insert(k.clone(), document_to_value(v));
            }
            Value::Object(obj)
        }
        Document::Array(arr) => Value::Array(arr.iter().map(document_to_value).collect()),
        Document::String(s) => Value::String(s.clone()),
        Document::Number(n) => match n {
            Number::PosInt(u) => Value::Number(serde_json::Number::from(*u)),
            Number::NegInt(i) => Value::Number(serde_json::Number::from(*i)),
            Number::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        },
        Document::Bool(b) => Value::Bool(*b),
        Document::Null => Value::Null,
    }
}

// ── Message conversion ──

fn to_bedrock_messages(messages: &[Message]) -> Result<Vec<BedrockMessage>> {
    messages
        .iter()
        .filter_map(|msg| {
            let role = match msg.role.as_str() {
                "user" => ConversationRole::User,
                "assistant" => ConversationRole::Assistant,
                _ => return None,
            };
            let content = convert_content_to_bedrock(&msg.content);
            match BedrockMessage::builder()
                .role(role)
                .set_content(if content.is_empty() {
                    None
                } else {
                    Some(content)
                })
                .build()
            {
                Ok(message) => Some(Ok(message)),
                Err(e) => Some(Err(anyhow!("failed to build Bedrock message: {e}"))),
            }
        })
        .collect()
}

fn convert_content_to_bedrock(content: &Value) -> Vec<BedrockContentBlock> {
    let mut blocks = Vec::new();

    if let Some(text) = content.as_str() {
        if !text.trim().is_empty() {
            blocks.push(BedrockContentBlock::Text(text.to_string()));
        }
        return blocks;
    }

    let Some(items) = content.as_array() else {
        return blocks;
    };

    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        blocks.push(BedrockContentBlock::Text(text.to_string()));
                    }
                }
            }
            Some("tool_use") => {
                let id = item["id"].as_str().unwrap_or_default().to_string();
                let name = item["name"].as_str().unwrap_or_default().to_string();
                let input = item
                    .get("input")
                    .map(value_to_document)
                    .unwrap_or(Document::Null);
                if let Ok(tool_use) = ToolUseBlock::builder()
                    .tool_use_id(id)
                    .name(name)
                    .input(input)
                    .build()
                {
                    blocks.push(BedrockContentBlock::ToolUse(tool_use));
                }
            }
            Some("tool_result") => {
                let tool_use_id = item["tool_use_id"].as_str().unwrap_or_default().to_string();
                let result_content = item["content"].as_str().unwrap_or("");
                let is_error = item["is_error"].as_bool().unwrap_or(false);
                if let Ok(tr) = ToolResultBlock::builder()
                    .tool_use_id(tool_use_id)
                    .set_content(Some(vec![ToolResultContentBlock::Text(
                        result_content.to_string(),
                    )]))
                    .set_status(if is_error {
                        Some(ToolResultStatus::Error)
                    } else {
                        Some(ToolResultStatus::Success)
                    })
                    .build()
                {
                    blocks.push(BedrockContentBlock::ToolResult(tr));
                }
            }
            _ => {}
        }
    }

    blocks
}

// ── Tool conversion ──

fn to_bedrock_tools(tools: &[Value]) -> Vec<Tool> {
    tools
        .iter()
        .map(|tool| {
            let name = tool["name"].as_str().unwrap_or("unknown").to_string();
            let description = tool["description"].as_str().unwrap_or("").to_string();
            let schema = value_to_document(&tool["input_schema"]);
            Tool::ToolSpec(
                ToolSpecification::builder()
                    .name(name)
                    .description(description)
                    .input_schema(ToolInputSchema::Json(schema))
                    .build()
                    .expect("valid tool spec"),
            )
        })
        .collect()
}

// ── Response conversion ──

fn from_bedrock_response(
    output: Option<aws_sdk_bedrockruntime::types::ConverseOutput>,
    stop_reason: StopReason,
    usage: Option<aws_sdk_bedrockruntime::types::TokenUsage>,
) -> Result<LlmResponse> {
    let message = match output {
        Some(aws_sdk_bedrockruntime::types::ConverseOutput::Message(msg)) => msg,
        _ => return Err(anyhow!("no message in Bedrock ConverseOutput")),
    };

    let mut content = Vec::new();

    for block in message.content {
        match block {
            BedrockContentBlock::Text(text) => {
                if !text.trim().is_empty() {
                    content.push(ContentBlock::Text { text });
                }
            }
            BedrockContentBlock::ToolUse(tool_use) => {
                let input = document_to_value(&tool_use.input);
                content.push(ContentBlock::ToolUse {
                    id: tool_use.tool_use_id,
                    name: tool_use.name,
                    input,
                });
            }
            _ => {}
        }
    }

    let reason_str = format!("{:?}", stop_reason);

    let usage = usage.map(|u| TokenUsage {
        input_tokens: u.input_tokens as u32,
        output_tokens: u.output_tokens as u32,
        cache_hit_tokens: 0,
        cache_miss_tokens: 0,
    });

    Ok(LlmResponse {
        content,
        stop_reason: Some(reason_str),
        usage,
    })
}

// ── LlmBackend impl ──

#[async_trait]
impl LlmBackend for BedrockBackend {
    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        let bedrock_messages = to_bedrock_messages(messages)?;

        let mut builder = self
            .client
            .converse()
            .model_id(&self.model_id)
            .set_messages(Some(bedrock_messages));

        if !tools.is_empty() {
            if let Ok(tool_config) = ToolConfiguration::builder()
                .set_tools(Some(to_bedrock_tools(tools)))
                .build()
            {
                builder = builder.tool_config(tool_config);
            }
        }

        builder = builder.inference_config(
            InferenceConfiguration::builder()
                .temperature(0.7)
                .max_tokens(DEFAULT_OUTPUT_TOKENS)
                .build(),
        );

        let output = builder.send().await.map_err(|err| {
            anyhow!(
                "Bedrock API error (region={}, model={}): {err}",
                self.region,
                self.model_id
            )
        })?;

        from_bedrock_response(output.output, output.stop_reason, output.usage)
    }

    async fn ask_streaming(
        &self,
        _messages: &[Message],
        _tools: &[Value],
        _on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        Err(anyhow!(
            "Bedrock streaming not yet implemented. Use non-streaming path."
        ))
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Err(anyhow!(
            "Bedrock embedding not yet supported. Use a different provider for embeddings."
        ))
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        let mut all_messages = messages.to_vec();
        all_messages.push(Message {
            role: "user".to_string(),
            content: json!([{ "type": "text", "text": instruction }]),
        });
        let response = self.ask(&all_messages, &[]).await?;
        for block in &response.content {
            if let ContentBlock::Text { text } = block {
                return Ok(text.clone());
            }
        }
        Ok(String::new())
    }

    fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
        let (window, max_output) = model_context_window(&self.model_id)
            .unwrap_or((DEFAULT_CONTEXT_WINDOW, DEFAULT_OUTPUT_TOKENS as usize));
        Some(ContextBudget {
            context_window_tokens: window,
            reserved_output_tokens: max_output,
            compact_threshold_tokens: window.saturating_sub(max_output),
        })
    }
}
