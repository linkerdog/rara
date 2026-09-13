//! Shared LLM types used by both providers and the agent loop.

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
    pub cache_hit_tokens: u32,
    #[serde(default)]
    pub cache_miss_tokens: u32,
}

/// The finished response returned by an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub usage: Option<TokenUsage>,
}
