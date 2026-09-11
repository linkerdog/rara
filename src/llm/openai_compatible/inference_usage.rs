use rara_observability::InferenceTokenUsage;
use serde_json::Value;

/// Normalize billing without changing the legacy runtime token counters.
/// Anthropic input excludes cache tokens; Responses/Chat input includes them.
pub(in crate::llm) fn parse_inference_usage(usage: &Value) -> Option<InferenceTokenUsage> {
    let mut input_tokens =
        count(usage, &["prompt_tokens"]).or_else(|| count(usage, &["input_tokens"]))?;
    let output_tokens =
        count(usage, &["completion_tokens"]).or_else(|| count(usage, &["output_tokens"]))?;
    let cache_read_tokens = count(usage, &["prompt_cache_hit_tokens"])
        .or_else(|| count(usage, &["cache_read_input_tokens"]))
        .or_else(|| count(usage, &["prompt_tokens_details", "cached_tokens"]))
        .or_else(|| count(usage, &["input_tokens_details", "cached_tokens"]));
    let creation = count(usage, &["cache_creation_input_tokens"]);
    let short = count(usage, &["cache_creation", "ephemeral_5m_input_tokens"]);
    let long = count(usage, &["cache_creation", "ephemeral_1h_input_tokens"]);
    let anthropic = usage.get("cache_read_input_tokens").is_some() || creation.is_some();
    let (cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens) = if anthropic {
        let created = creation?;
        input_tokens = input_tokens
            .checked_add(cache_read_tokens?)?
            .checked_add(created)?;
        match (short, long) {
            (Some(short), Some(long)) => (
                Some(created.checked_sub(short.checked_add(long)?)?),
                Some(short),
                Some(long),
            ),
            // An undifferentiated write is priced only by an explicit generic-write tariff.
            (None, None) => (Some(created), Some(0), Some(0)),
            _ => (None, short, long),
        }
    } else {
        let writes = count(usage, &["input_tokens_details", "cache_write_tokens"])
            .or_else(|| count(usage, &["prompt_tokens_details", "cache_write_tokens"]))
            .or_else(|| usage.get("prompt_cache_miss_tokens").map(|_| 0));
        (writes, Some(0), Some(0))
    };
    Some(InferenceTokenUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cache_write_5m_tokens,
        cache_write_1h_tokens,
    })
}

fn count(usage: &Value, path: &[&str]) -> Option<u64> {
    let mut value = usage;
    for key in path {
        value = value.get(*key)?;
    }
    value.as_u64()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn anthropic_input_is_inclusive_and_ttl_writes_are_disjoint() {
        let usage = parse_inference_usage(&json!({
            "input_tokens": 200, "output_tokens": 100,
            "cache_read_input_tokens": 600, "cache_creation_input_tokens": 200,
            "cache_creation": {"ephemeral_5m_input_tokens": 100, "ephemeral_1h_input_tokens": 100}
        }))
        .unwrap();
        assert_eq!(
            usage,
            InferenceTokenUsage {
                input_tokens: 1_000,
                output_tokens: 100,
                cache_read_tokens: Some(600),
                cache_write_tokens: Some(0),
                cache_write_5m_tokens: Some(100),
                cache_write_1h_tokens: Some(100),
            }
        );
    }

    #[test]
    fn responses_cache_writes_are_not_uncached_input() {
        let usage = parse_inference_usage(&json!({
            "input_tokens": 15000, "output_tokens": 10,
            "input_tokens_details": {"cached_tokens": 12000, "cache_write_tokens": 3000}
        }))
        .unwrap();
        assert_eq!(usage.input_tokens, 15000);
        assert_eq!(usage.cache_read_tokens, Some(12000));
        assert_eq!(usage.cache_write_tokens, Some(3000));
    }

    #[test]
    fn absent_usage_is_not_zero_and_explicit_zero_cache_is_preserved() {
        assert!(parse_inference_usage(&json!({"input_tokens": 20})).is_none());
        let missing =
            parse_inference_usage(&json!({"input_tokens": 20, "output_tokens": 1})).unwrap();
        assert_eq!(missing.cache_read_tokens, None);
        let zero = parse_inference_usage(&json!({
            "prompt_tokens": 20, "completion_tokens": 1, "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": 20
        }))
        .unwrap();
        assert_eq!(zero.cache_read_tokens, Some(0));
    }
}
