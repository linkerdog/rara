// Re-export shared types from rara-core.
pub use rara_core::llm::types::{ContentBlock, Message};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Kept locally until field names are aligned with rara-core.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LlmResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_hit_tokens: u32,
    #[serde(default)]
    pub cache_miss_tokens: u32,
}
