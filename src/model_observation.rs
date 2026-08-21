//! Structured, content-free observations for model requests.

use serde::{Deserialize, Serialize};

use crate::llm::TokenUsage;

/// Provider-reported prompt-cache token accounting for one model request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCacheUsage {
    pub hit_tokens: u32,
    pub miss_tokens: u32,
}

impl ModelCacheUsage {
    /// Return the provider-reported cache hit rate in basis points.
    pub fn hit_rate_basis_points(self) -> Option<u16> {
        let total = u64::from(self.hit_tokens).saturating_add(u64::from(self.miss_tokens));
        if total == 0 {
            return None;
        }
        let basis_points = u64::from(self.hit_tokens)
            .saturating_mul(10_000)
            .checked_div(total)?;
        Some(basis_points.min(u64::from(u16::MAX)) as u16)
    }
}

/// Token usage returned for one model request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// `None` means the provider did not expose usable cache accounting for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<ModelCacheUsage>,
}

impl ModelTokenUsage {
    pub(crate) fn from_provider_usage(usage: &TokenUsage) -> Self {
        let cache = (usage.cache_hit_tokens != 0 || usage.cache_miss_tokens != 0).then_some(
            ModelCacheUsage {
                hit_tokens: usage.cache_hit_tokens,
                miss_tokens: usage.cache_miss_tokens,
            },
        );
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache,
        }
    }
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

/// Observation for one main-model request in an agent query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTurnReport {
    pub model: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelTokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_fingerprint: Option<ModelRequestFingerprint>,
}

/// Structured observations collected while executing one user query.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryReport {
    pub model_turns: Vec<ModelTurnReport>,
}

#[cfg(test)]
mod tests {
    use super::{ModelCacheUsage, ModelRequestFingerprint, ModelTokenUsage};
    use crate::llm::TokenUsage;

    #[test]
    fn cache_hit_rate_uses_basis_points() {
        assert_eq!(
            ModelCacheUsage {
                hit_tokens: 3,
                miss_tokens: 1,
            }
            .hit_rate_basis_points(),
            Some(7_500)
        );
        assert_eq!(ModelCacheUsage::default().hit_rate_basis_points(), None);
    }

    #[test]
    fn provider_usage_distinguishes_zero_hits_from_missing_cache_accounting() {
        let zero_hit = ModelTokenUsage::from_provider_usage(&TokenUsage {
            input_tokens: 100,
            output_tokens: 12,
            cache_hit_tokens: 0,
            cache_miss_tokens: 100,
        });
        let missing = ModelTokenUsage::from_provider_usage(&TokenUsage {
            input_tokens: 100,
            output_tokens: 12,
            cache_hit_tokens: 0,
            cache_miss_tokens: 0,
        });

        assert_eq!(
            zero_hit.cache,
            Some(ModelCacheUsage {
                hit_tokens: 0,
                miss_tokens: 100,
            })
        );
        assert_eq!(missing.cache, None);
    }

    #[test]
    fn fingerprint_compares_bounded_message_prefixes() {
        let fingerprint = |messages: &[&str]| ModelRequestFingerprint {
            version: 1,
            hash_scope: "scope".to_string(),
            request_sha256: "request".to_string(),
            system_sha256: None,
            messages_sha256: "messages".to_string(),
            tools_sha256: "tools".to_string(),
            options_sha256: "options".to_string(),
            message_sha256: messages.iter().map(|value| (*value).to_string()).collect(),
            tool_sha256: Vec::new(),
            message_count: messages.len(),
            tool_count: 0,
        };

        assert_eq!(
            fingerprint(&["system", "user", "assistant"])
                .shared_message_prefix_len(&fingerprint(&["system", "user", "next"])),
            2
        );
    }
}
