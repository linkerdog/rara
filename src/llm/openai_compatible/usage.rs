use serde_json::Value;

use crate::llm::TokenUsage;

pub(super) fn parse_openai_token_usage(usage: &Value) -> TokenUsage {
    let input_tokens = first_u32(
        usage,
        &[
            &["prompt_tokens"],
            &["input_tokens"],
            &["usage", "prompt_tokens"],
            &["usage", "input_tokens"],
        ],
    );
    let output_tokens = first_u32(
        usage,
        &[
            &["completion_tokens"],
            &["output_tokens"],
            &["usage", "completion_tokens"],
            &["usage", "output_tokens"],
        ],
    );
    let cache_hit_paths: &[&[&str]] = &[
        &["prompt_cache_hit_tokens"],
        &["cache_read_input_tokens"],
        &["prompt_tokens_details", "cached_tokens"],
        &["input_tokens_details", "cached_tokens"],
    ];
    let cache_miss_paths: &[&[&str]] = &[
        &["prompt_cache_miss_tokens"],
        &["cache_creation_input_tokens"],
        &["prompt_tokens_details", "uncached_tokens"],
        &["input_tokens_details", "uncached_tokens"],
    ];
    let cache_hit_value = first_u32_value(usage, cache_hit_paths);
    let explicit_cache_miss_value = first_u32_value(usage, cache_miss_paths);
    let cache_hit_tokens = cache_hit_value.unwrap_or(0);
    let explicit_cache_miss_tokens = explicit_cache_miss_value.unwrap_or(0);
    let has_cache_accounting = cache_hit_value.is_some() || explicit_cache_miss_value.is_some();
    let cache_miss_tokens = if explicit_cache_miss_value.is_some() {
        explicit_cache_miss_tokens
    } else if has_cache_accounting {
        input_tokens.saturating_sub(cache_hit_tokens)
    } else {
        0
    };

    TokenUsage {
        input_tokens,
        output_tokens,
        cache_hit_tokens,
        cache_miss_tokens,
    }
}

fn first_u32(value: &Value, paths: &[&[&str]]) -> u32 {
    first_u32_value(value, paths).unwrap_or(0)
}

fn first_u32_value(value: &Value, paths: &[&[&str]]) -> Option<u32> {
    paths
        .iter()
        .find_map(|path| nested_u64(value, path))
        .map(|tokens| tokens.min(u32::MAX as u64) as u32)
}

fn nested_u64(value: &Value, path: &[&str]) -> Option<u64> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_u64()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_openai_token_usage;

    #[test]
    fn parses_openai_prompt_details_cached_tokens() {
        let usage = parse_openai_token_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 12,
            "prompt_tokens_details": {
                "cached_tokens": 60
            }
        }));

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.cache_hit_tokens, 60);
        assert_eq!(usage.cache_miss_tokens, 40);
    }

    #[test]
    fn parses_provider_explicit_cache_hit_and_miss_tokens() {
        let usage = parse_openai_token_usage(&json!({
            "input_tokens": 100,
            "output_tokens": 12,
            "prompt_cache_hit_tokens": 70,
            "prompt_cache_miss_tokens": 25
        }));

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.cache_hit_tokens, 70);
        assert_eq!(usage.cache_miss_tokens, 25);
    }

    #[test]
    fn treats_zero_cached_tokens_as_present_cache_accounting() {
        let usage = parse_openai_token_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 12,
            "prompt_tokens_details": {
                "cached_tokens": 0
            }
        }));

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.cache_hit_tokens, 0);
        assert_eq!(usage.cache_miss_tokens, 100);
    }

    #[test]
    fn leaves_cache_accounting_absent_when_provider_omits_cache_fields() {
        let usage = parse_openai_token_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 12
        }));

        assert_eq!(usage.cache_hit_tokens, 0);
        assert_eq!(usage.cache_miss_tokens, 0);
    }
}
