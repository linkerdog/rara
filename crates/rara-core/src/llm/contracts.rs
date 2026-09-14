//! Provider-neutral LLM contract types shared across backends and hosts.
//!
//! These types are intentionally free of async-runtime and transport
//! dependencies so the core crate stays portable to `wasm32-unknown-unknown`.

use serde::{Deserialize, Serialize};

/// Token budget boundaries used to decide when a provider context should compact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub context_window_tokens: usize,
    pub reserved_output_tokens: usize,
    pub compact_threshold_tokens: usize,
}

/// Provider capabilities relevant to prompt-cache accounting and editing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderCacheProfile {
    pub automatic_prefix_cache: bool,
    pub explicit_prefix_cache: bool,
    pub cache_usage_accounting: bool,
    pub cache_edit: bool,
    pub cache_retention_control: bool,
}

impl ProviderCacheProfile {
    pub const fn none() -> Self {
        Self {
            automatic_prefix_cache: false,
            explicit_prefix_cache: false,
            cache_usage_accounting: false,
            cache_edit: false,
            cache_retention_control: false,
        }
    }

    pub const fn automatic_prefix_cache_with_usage() -> Self {
        Self {
            automatic_prefix_cache: true,
            explicit_prefix_cache: false,
            cache_usage_accounting: true,
            cache_edit: false,
            cache_retention_control: false,
        }
    }
}

/// Execution mode for a single LLM turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmExecutionMode {
    Execute,
    Plan,
}

/// Incremental events emitted while a backend streams a completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmStreamEvent {
    TextDelta(String),
    ReasoningDelta(String),
}

/// Experiment selection for summary routing; the default preserves
/// auxiliary-model routing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummaryStrategy {
    #[default]
    AuxiliaryModel,
    CachedMainModel,
}

/// Content-free hashes of provider request components that affect cache locality.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequestFingerprint {
    /// Version of the fingerprint layout, independent from the hash algorithm.
    pub version: u8,
    /// Opaque scope in which hashes are comparable. The corresponding salt is never reported.
    pub hash_scope: String,
    /// SHA-256 of the complete logical request body, excluding transport-only fields.
    pub request_sha256: String,
    /// SHA-256 of all leading system messages, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_sha256: Option<String>,
    /// SHA-256 of the serialized provider message list.
    pub messages_sha256: String,
    /// SHA-256 of the serialized provider tool list.
    pub tools_sha256: String,
    /// SHA-256 of model and other request options.
    pub options_sha256: String,
    /// A bounded prefix of per-message SHA-256 values for prefix comparison.
    pub message_sha256: Vec<String>,
    /// A bounded prefix of per-tool SHA-256 values for tool-schema comparison.
    pub tool_sha256: Vec<String>,
    pub message_count: usize,
    pub tool_count: usize,
}

impl ModelRequestFingerprint {
    /// Count the identical leading provider messages shared with another request.
    pub fn shared_message_prefix_len(&self, other: &Self) -> usize {
        if self.hash_scope != other.hash_scope {
            return 0;
        }
        self.message_sha256
            .iter()
            .zip(&other.message_sha256)
            .take_while(|(left, right)| left == right)
            .count()
    }
}
