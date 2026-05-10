use anyhow::{Result, anyhow};
use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::types::{
    ContentBlock as BedrockContentBlock, ConversationRole, InferenceConfiguration,
    Message as BedrockMessage, StopReason, SystemContentBlock, Tool, ToolConfiguration,
    ToolInputSchema, ToolResultBlock, ToolResultContentBlock, ToolResultStatus, ToolSpecification,
    ToolUseBlock,
};
use aws_smithy_types::{Document, Number};
use serde_json::Value;

const DEFAULT_CONTEXT_WINDOW: usize = 200_000;
const DEFAULT_OUTPUT_TOKENS: i32 = 8_192;

#[derive(Debug, Clone, PartialEq)]
pub struct BedrockChatMessage {
    pub role: BedrockChatRole,
    pub content: Vec<BedrockChatContent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BedrockChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BedrockChatContent {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BedrockToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BedrockChatResponse {
    pub content: Vec<BedrockResponseContent>,
    pub stop_reason: Option<String>,
    pub usage: Option<BedrockTokenUsage>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BedrockResponseContent {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BedrockTokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

pub struct BedrockConverseClient {
    client: BedrockClient,
    model_id: String,
    region: String,
}

impl BedrockConverseClient {
    pub async fn new(region: Option<String>, model_id: String) -> Result<Self> {
        let mut config_loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(region) = region.as_deref() {
            config_loader = config_loader.region(aws_config::Region::new(region.to_string()));
        }
        let sdk_config = config_loader.load().await;
        let client = BedrockClient::new(&sdk_config);

        Ok(Self {
            client,
            model_id,
            region: region.unwrap_or_else(|| "default".to_string()),
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn ask(
        &self,
        system: &[String],
        messages: &[BedrockChatMessage],
        tools: &[BedrockToolSpec],
    ) -> Result<BedrockChatResponse> {
        let bedrock_messages = to_bedrock_messages(messages)?;

        let mut builder = self
            .client
            .converse()
            .model_id(&self.model_id)
            .set_messages(Some(bedrock_messages));

        if !system.is_empty() {
            let system_blocks: Vec<SystemContentBlock> = system
                .iter()
                .map(|s| SystemContentBlock::Text(s.clone()))
                .collect();
            builder = builder.set_system(Some(system_blocks));
        }

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
}

pub fn model_context_window(model_id: &str) -> (usize, usize) {
    let lower = model_id.to_lowercase();
    let known = if lower.contains("claude-sonnet-4") || lower.contains("claude-3-5-sonnet") {
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
    };

    known.unwrap_or((DEFAULT_CONTEXT_WINDOW, DEFAULT_OUTPUT_TOKENS as usize))
}

fn value_to_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(value) => Document::Bool(*value),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Document::Number(Number::NegInt(value))
            } else if let Some(value) = value.as_u64() {
                Document::Number(Number::PosInt(value))
            } else if let Some(value) = value.as_f64() {
                Document::Number(Number::Float(value))
            } else {
                Document::Null
            }
        }
        Value::String(value) => Document::String(value.clone()),
        Value::Array(values) => Document::Array(values.iter().map(value_to_document).collect()),
        Value::Object(values) => Document::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value_to_document(value)))
                .collect(),
        ),
    }
}

fn document_to_value(document: &Document) -> Value {
    match document {
        Document::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), document_to_value(value)))
                .collect(),
        ),
        Document::Array(values) => Value::Array(values.iter().map(document_to_value).collect()),
        Document::String(value) => Value::String(value.clone()),
        Document::Number(value) => match value {
            Number::PosInt(value) => Value::Number(serde_json::Number::from(*value)),
            Number::NegInt(value) => Value::Number(serde_json::Number::from(*value)),
            Number::Float(value) => serde_json::Number::from_f64(*value)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        },
        Document::Bool(value) => Value::Bool(*value),
        Document::Null => Value::Null,
    }
}

fn to_bedrock_messages(messages: &[BedrockChatMessage]) -> Result<Vec<BedrockMessage>> {
    messages
        .iter()
        .map(|message| {
            let role = match message.role {
                BedrockChatRole::User => ConversationRole::User,
                BedrockChatRole::Assistant => ConversationRole::Assistant,
            };
            BedrockMessage::builder()
                .role(role)
                .set_content(if message.content.is_empty() {
                    None
                } else {
                    Some(convert_content_to_bedrock(&message.content))
                })
                .build()
                .map_err(|err| anyhow!("failed to build Bedrock message: {err}"))
        })
        .collect()
}

fn convert_content_to_bedrock(content: &[BedrockChatContent]) -> Vec<BedrockContentBlock> {
    let mut blocks = Vec::new();

    for item in content {
        match item {
            BedrockChatContent::Text(text) => {
                if !text.trim().is_empty() {
                    blocks.push(BedrockContentBlock::Text(text.clone()));
                }
            }
            BedrockChatContent::ToolUse { id, name, input } => {
                if let Ok(tool_use) = ToolUseBlock::builder()
                    .tool_use_id(id.clone())
                    .name(name.clone())
                    .input(value_to_document(input))
                    .build()
                {
                    blocks.push(BedrockContentBlock::ToolUse(tool_use));
                }
            }
            BedrockChatContent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                if let Ok(tool_result) = ToolResultBlock::builder()
                    .tool_use_id(tool_use_id.clone())
                    .set_content(Some(vec![ToolResultContentBlock::Text(content.clone())]))
                    .set_status(if *is_error {
                        Some(ToolResultStatus::Error)
                    } else {
                        Some(ToolResultStatus::Success)
                    })
                    .build()
                {
                    blocks.push(BedrockContentBlock::ToolResult(tool_result));
                }
            }
        }
    }

    blocks
}

fn to_bedrock_tools(tools: &[BedrockToolSpec]) -> Vec<Tool> {
    tools
        .iter()
        .map(|tool| {
            let tool_name = tool.name.clone();
            Tool::ToolSpec(
                ToolSpecification::builder()
                    .name(tool.name.clone())
                    .description(tool.description.clone())
                    .input_schema(ToolInputSchema::Json(value_to_document(&tool.input_schema)))
                    .build()
                    .unwrap_or_else(|_| panic!("invalid tool spec: {tool_name}")),
            )
        })
        .collect()
}

fn from_bedrock_response(
    output: Option<aws_sdk_bedrockruntime::types::ConverseOutput>,
    stop_reason: StopReason,
    usage: Option<aws_sdk_bedrockruntime::types::TokenUsage>,
) -> Result<BedrockChatResponse> {
    let message = match output {
        Some(aws_sdk_bedrockruntime::types::ConverseOutput::Message(message)) => message,
        _ => return Err(anyhow!("no message in Bedrock ConverseOutput")),
    };

    let mut content = Vec::new();
    for block in message.content {
        match block {
            BedrockContentBlock::Text(text) => {
                if !text.trim().is_empty() {
                    content.push(BedrockResponseContent::Text(text));
                }
            }
            BedrockContentBlock::ToolUse(tool_use) => {
                content.push(BedrockResponseContent::ToolUse {
                    id: tool_use.tool_use_id,
                    name: tool_use.name,
                    input: document_to_value(&tool_use.input),
                });
            }
            _ => {}
        }
    }

    Ok(BedrockChatResponse {
        content,
        stop_reason: Some(format!("{stop_reason:?}")),
        usage: usage.map(|usage| BedrockTokenUsage {
            input_tokens: usage.input_tokens as u32,
            output_tokens: usage.output_tokens as u32,
        }),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn model_context_window_uses_bedrock_family_defaults() {
        assert_eq!(
            model_context_window("us.anthropic.claude-3-5-sonnet-20241022-v2:0"),
            (200_000, 8_192)
        );
        assert_eq!(
            model_context_window("us.amazon.nova-pro-v1:0"),
            (300_000, 5_000)
        );
        assert_eq!(model_context_window("unknown-model"), (200_000, 8_192));
    }

    #[test]
    fn document_conversion_roundtrips_json_values() {
        let value = json!({
            "string": "value",
            "bool": true,
            "int": -7,
            "uint": 7_u64,
            "float": 1.25,
            "array": [null, "x"]
        });

        assert_eq!(document_to_value(&value_to_document(&value)), value);
    }

    #[test]
    fn content_conversion_skips_blank_text() {
        let blocks = convert_content_to_bedrock(&[
            BedrockChatContent::Text("  ".to_string()),
            BedrockChatContent::Text("hello".to_string()),
        ]);

        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], BedrockContentBlock::Text(text) if text == "hello"));
    }
}
