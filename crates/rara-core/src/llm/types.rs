//! Shared LLM types used by both providers and the agent loop.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single chat message.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: Value,
}

/// A block within a completion response.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "provider_metadata")]
    ProviderMetadata {
        provider: String,
        key: String,
        value: Value,
    },
}

/// Token usage summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
}

impl TokenUsage {
    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

impl std::ops::AddAssign for TokenUsage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        merge_opt(&mut self.cache_read_input_tokens, other.cache_read_input_tokens);
        merge_opt(
            &mut self.cache_creation_input_tokens,
            other.cache_creation_input_tokens,
        );
        merge_opt(&mut self.reasoning_tokens, other.reasoning_tokens);
    }
}

fn merge_opt(a: &mut Option<u32>, b: Option<u32>) {
    if let Some(b) = b {
        *a = Some(a.unwrap_or(0) + b);
    }
}

/// The finished response returned by an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: Vec<ContentBlock>,
    pub usage: TokenUsage,
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub extra: HashMap<String, Value>,
}

impl LlmResponse {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn tool_uses(&self) -> Vec<&ContentBlock> {
        self.content
            .iter()
            .filter(|block| matches!(block, ContentBlock::ToolUse { .. }))
            .collect()
    }
}
