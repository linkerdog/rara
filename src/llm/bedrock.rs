use anyhow::{Result, anyhow};
use async_trait::async_trait;
use rara_bedrock::{
    BedrockCacheTtl, BedrockChatContent, BedrockChatMessage, BedrockChatRole,
    BedrockConverseClient, BedrockRequestOptions, BedrockResponseContent, BedrockToolSpec,
    model_context_window,
};
use serde_json::{Value, json};

use super::shared::{ContextBudget, LlmBackend, LlmTurnMetadata, ProviderCacheProfile};
use crate::agent::Message;
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};
use crate::model_context::{MODEL_CONTEXT_BLOCK_TYPE, model_context_text};

pub struct BedrockBackend {
    client: BedrockConverseClient,
    cache_ttl: Option<BedrockCacheTtl>,
}

impl BedrockBackend {
    pub async fn new(region: Option<String>, model_id: String) -> Result<Self> {
        Ok(Self {
            cache_ttl: rara_bedrock::supports_claude_cache(&model_id)
                .then_some(BedrockCacheTtl::FiveMinutes),
            client: BedrockConverseClient::new(region, model_id).await?,
        })
    }
}

fn cache_profile_for_ttl(ttl: Option<BedrockCacheTtl>) -> ProviderCacheProfile {
    ProviderCacheProfile {
        explicit_prefix_cache: ttl.is_some(),
        cache_usage_accounting: ttl.is_some(),
        cache_retention_control: ttl.is_some(),
        ..ProviderCacheProfile::none()
    }
}

fn to_bedrock_messages(messages: &[Message]) -> Vec<BedrockChatMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role.as_str() {
                "user" => BedrockChatRole::User,
                "assistant" => BedrockChatRole::Assistant,
                _ => return None,
            };
            Some(BedrockChatMessage {
                role,
                content: convert_content_to_bedrock(&message.content),
            })
        })
        .collect()
}

fn convert_content_to_bedrock(content: &Value) -> Vec<BedrockChatContent> {
    if let Some(text) = content.as_str() {
        return vec![BedrockChatContent::Text(text.to_string())];
    }

    let Some(items) = content.as_array() else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("text") => item
                .get("text")
                .and_then(Value::as_str)
                .map(|text| BedrockChatContent::Text(text.to_string())),
            Some(MODEL_CONTEXT_BLOCK_TYPE) => {
                model_context_text(item).map(|text| BedrockChatContent::Text(text.to_string()))
            }
            Some("tool_use") => Some(BedrockChatContent::ToolUse {
                id: item["id"].as_str().unwrap_or_default().to_string(),
                name: item["name"].as_str().unwrap_or_default().to_string(),
                input: item.get("input").cloned().unwrap_or(Value::Null),
            }),
            Some("tool_result") => Some(BedrockChatContent::ToolResult {
                tool_use_id: item["tool_use_id"].as_str().unwrap_or_default().to_string(),
                content: item["content"].as_str().unwrap_or("").to_string(),
                is_error: item["is_error"].as_bool().unwrap_or(false),
            }),
            _ => None,
        })
        .collect()
}

fn to_bedrock_tools(tools: &[Value]) -> Vec<BedrockToolSpec> {
    tools
        .iter()
        .map(|tool| BedrockToolSpec {
            name: tool["name"].as_str().unwrap_or("unknown").to_string(),
            description: tool["description"].as_str().unwrap_or("").to_string(),
            input_schema: tool["input_schema"].clone(),
        })
        .collect()
}

fn extract_system_prompt(messages: &[Message]) -> (Vec<String>, Vec<Message>) {
    let mut system = Vec::new();
    let mut other = Vec::new();
    for message in messages {
        if message.role == "system" {
            if let Some(text) = message.content.as_str() {
                system.push(text.to_string());
            } else if let Some(items) = message.content.as_array() {
                for item in items {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        system.push(text.to_string());
                    }
                }
            }
        } else {
            other.push(message.clone());
        }
    }
    (system, other)
}

#[async_trait]
impl LlmBackend for BedrockBackend {
    async fn summarize_with_prefix(
        &self,
        messages: &[Message],
        instruction: &str,
        prefix: &super::SummaryPrefix,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        let messages = prefix.messages_for_summary(messages, instruction)?;
        let response = self
            .ask_with_context(&messages, &prefix.tools, metadata)
            .await?;
        super::summary::summary_text(response)
    }
    fn cache_profile(&self) -> ProviderCacheProfile {
        cache_profile_for_ttl(self.cache_ttl)
    }
    fn model_label(&self) -> Option<String> {
        Some(self.client.model_id().to_string())
    }

    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        self.ask_with_context(messages, tools, LlmTurnMetadata::default())
            .await
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        _on_event: &mut (dyn FnMut(super::LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask_with_context(messages, tools, metadata).await
    }

    async fn ask_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
    ) -> Result<LlmResponse> {
        metadata.ensure_not_cancelled()?;
        let (system, messages) = extract_system_prompt(messages);
        let response = self
            .client
            .ask_with_options(
                &system,
                &to_bedrock_messages(&messages),
                &to_bedrock_tools(tools),
                BedrockRequestOptions {
                    cache_ttl: self.cache_ttl,
                    inference: metadata.inference(),
                },
            )
            .await?;

        let mut content = Vec::new();
        for block in response.content {
            match block {
                BedrockResponseContent::Text(text) => {
                    content.push(ContentBlock::Text { text });
                }
                BedrockResponseContent::ToolUse { id, name, input } => {
                    content.push(ContentBlock::ToolUse { id, name, input });
                }
            }
        }

        Ok(LlmResponse {
            content,
            stop_reason: response.stop_reason,
            usage: response.usage.map(|usage| TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_hit_tokens: 0,
                cache_miss_tokens: 0,
            }),
        })
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        self.summarize_with_context(messages, instruction, LlmTurnMetadata::default())
            .await
    }

    async fn summarize_with_context(
        &self,
        messages: &[Message],
        instruction: &str,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        let mut all_messages = messages.to_vec();
        all_messages.push(Message {
            role: "user".to_string(),
            content: json!([{ "type": "text", "text": instruction }]),
        });
        let response = self.ask_with_context(&all_messages, &[], metadata).await?;
        for block in &response.content {
            if let ContentBlock::Text { text } = block {
                return Ok(text.clone());
            }
        }
        Ok(String::new())
    }

    async fn classify_with_context(
        &self,
        instructions: &str,
        messages: &[Message],
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        let mut messages = messages.to_vec();
        messages.insert(
            0,
            Message {
                role: "system".into(),
                content: json!(instructions),
            },
        );
        self.summarize_with_context(&messages, instructions, metadata)
            .await
    }

    fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
        let (window, max_output) = model_context_window(self.client.model_id());
        Some(ContextBudget {
            context_window_tokens: window,
            reserved_output_tokens: max_output,
            compact_threshold_tokens: window.saturating_sub(max_output),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn inference_cache_capabilities_require_an_explicit_ttl() {
        assert_eq!(cache_profile_for_ttl(None), ProviderCacheProfile::none());
        for ttl in [BedrockCacheTtl::FiveMinutes, BedrockCacheTtl::OneHour] {
            let profile = cache_profile_for_ttl(Some(ttl));
            assert!(profile.explicit_prefix_cache);
            assert!(profile.cache_usage_accounting);
            assert!(profile.cache_retention_control);
            assert!(!profile.automatic_prefix_cache);
            assert!(!profile.cache_edit);
        }
    }

    #[test]
    fn converts_rara_messages_to_bedrock_messages() {
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: json!("ignored"),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type": "rara_model_context", "kind": "environment", "text": "workspace context"},
                    {"type": "text", "text": "hello"},
                    {"type": "tool_result", "tool_use_id": "call-1", "content": "ok", "is_error": false}
                ]),
            },
        ];

        let converted = to_bedrock_messages(&messages);

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, BedrockChatRole::User);
        assert_eq!(
            converted[0].content,
            vec![
                BedrockChatContent::Text("workspace context".to_string()),
                BedrockChatContent::Text("hello".to_string()),
                BedrockChatContent::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                },
            ]
        );
    }

    #[test]
    fn converts_rara_tool_schemas_to_bedrock_tool_specs() {
        let tools = vec![json!({
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type": "object"}
        })];

        let converted = to_bedrock_tools(&tools);

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].name, "read_file");
        assert_eq!(converted[0].description, "Read a file");
        assert_eq!(converted[0].input_schema, json!({"type": "object"}));
    }
}
