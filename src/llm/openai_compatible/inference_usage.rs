use rara_observability::InferenceTokenUsage;
use serde_json::Value;

/// Normalize billing without changing the legacy runtime token counters.
/// Anthropic input excludes cache tokens; Responses/Chat input includes them.
pub(in crate::llm) fn parse_inference_usage(usage: &Value) -> Option<InferenceTokenUsage> {
    let mut input_tokens =
        count(usage, &["prompt_tokens"]).or_else(|| count(usage, &["input_tokens"]))?;
    let output_tokens =
        count(usage, &["completion_tokens"]).or_else(|| count(usage, &["output_tokens"]))?;
    let mut input_tokens_incomplete = false;
    let cache_read_tokens = count(usage, &["prompt_cache_hit_tokens"])
        .or_else(|| count(usage, &["cache_read_input_tokens"]))
        .or_else(|| count(usage, &["prompt_tokens_details", "cached_tokens"]))
        .or_else(|| count(usage, &["input_tokens_details", "cached_tokens"]));
    let creation = count(usage, &["cache_creation_input_tokens"]);
    let short = count(usage, &["cache_creation", "ephemeral_5m_input_tokens"]);
    let long = count(usage, &["cache_creation", "ephemeral_1h_input_tokens"]);
    let unknown_creation = usage
        .get("cache_creation")
        .and_then(Value::as_object)
        .is_some_and(|details| {
            details.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "ephemeral_5m_input_tokens" | "ephemeral_1h_input_tokens"
                )
            })
        });
    let anthropic = usage.get("cache_read_input_tokens").is_some()
        || usage.get("cache_creation_input_tokens").is_some()
        || usage.get("cache_creation").is_some();
    let (cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens) = if anthropic {
        // Keep a lower bound on inclusive input when a category is missing.
        // The corresponding None still prevents pricing this as a complete bill.
        let known_created = creation.or_else(|| short.unwrap_or(0).checked_add(long.unwrap_or(0)));
        let inclusive = known_created.and_then(|created| {
            input_tokens
                .checked_add(cache_read_tokens.unwrap_or(0))?
                .checked_add(created)
        });
        input_tokens_incomplete =
            inclusive.is_none() || cache_read_tokens.is_none() || creation.is_none();
        input_tokens = inclusive.unwrap_or(input_tokens);
        match (short, long) {
            (Some(short), Some(long)) => (
                match creation {
                    Some(created) if !unknown_creation => short
                        .checked_add(long)
                        .and_then(|details| created.checked_sub(details)),
                    Some(_) | None => None,
                },
                Some(short),
                Some(long),
            ),
            // An undifferentiated write is priced only by an explicit generic-write tariff.
            (None, None) => (creation.filter(|_| !unknown_creation), None, None),
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
        input_tokens_incomplete,
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
    fn inference_usage_completeness_roundtrips_partial_and_legacy_totals() {
        let complete = parse_inference_usage(&json!({
            "prompt_tokens": 20, "completion_tokens": 1,
            "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 20
        }))
        .unwrap();
        let mut legacy = serde_json::to_value(complete).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("input_tokens_incomplete");
        assert_eq!(
            serde_json::from_value::<InferenceTokenUsage>(legacy).unwrap(),
            complete
        );
        let partial = InferenceTokenUsage {
            input_tokens_incomplete: true,
            ..complete
        };
        assert_eq!(
            serde_json::from_value::<InferenceTokenUsage>(serde_json::to_value(partial).unwrap())
                .unwrap(),
            partial
        );
    }

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
                input_tokens_incomplete: false,
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
    fn inconsistent_anthropic_totals_do_not_discard_known_usage() {
        for (base, read, creation, short, long, input, incomplete, generic, known_cost) in [
            (
                u64::MAX,
                1,
                Some(0),
                0,
                0,
                u64::MAX,
                true,
                Some(0),
                0.000101,
            ),
            (200, 10, Some(20), 40, 50, 230, false, None, 0.0002),
            (
                200,
                u64::MAX,
                Some(5),
                2,
                3,
                200,
                true,
                Some(0),
                u64::MAX as f64 / 1_000_000.0,
            ),
            (
                200,
                0,
                None,
                u64::MAX,
                1,
                200,
                true,
                None,
                u64::MAX as f64 / 1_000_000.0,
            ),
            (
                200,
                0,
                Some(u64::MAX),
                u64::MAX,
                1,
                200,
                true,
                None,
                u64::MAX as f64 / 1_000_000.0,
            ),
        ] {
            let receipt = json!({
                "input_tokens": base, "output_tokens": 100,
                "cache_read_input_tokens": read, "cache_creation_input_tokens": creation,
                "cache_creation": {"ephemeral_5m_input_tokens": short, "ephemeral_1h_input_tokens": long}
            });
            let usage = parse_inference_usage(&receipt).expect("known usage must survive");
            assert_eq!(
                usage,
                InferenceTokenUsage {
                    input_tokens: input,
                    input_tokens_incomplete: incomplete,
                    output_tokens: 100,
                    cache_read_tokens: Some(read),
                    cache_write_tokens: generic,
                    cache_write_5m_tokens: Some(short),
                    cache_write_1h_tokens: Some(long),
                }
            );
            let cost = price_usage(usage);
            assert!(!cost.complete);
            assert_eq!(cost.unpriced_attempts, 1);
            assert!(
                (cost.known_cost_usd - known_cost).abs() <= 1e-12 * known_cost.max(1.0),
                "{cost:?}"
            );
        }
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
    #[test]
    fn partial_anthropic_cache_categories_retain_known_usage() {
        for (fields, input, read, write, short, long, incomplete) in [
            (
                json!({
                    "cache_read_input_tokens": 10,
                    "cache_creation_input_tokens": 90,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 40,
                        "ephemeral_1h_input_tokens": 0,
                        "ephemeral_24h_input_tokens": 50
                    }
                }),
                300,
                Some(10),
                None,
                Some(40),
                Some(0),
                false,
            ),
            (
                json!({
                    "cache_read_input_tokens": 10,
                    "cache_creation_input_tokens": 50,
                    "cache_creation": {"ephemeral_24h_input_tokens": 50}
                }),
                260,
                Some(10),
                None,
                None,
                None,
                false,
            ),
            (
                json!({"cache_read_input_tokens":600}),
                800,
                Some(600),
                None,
                None,
                None,
                true,
            ),
            (
                json!({"cache_creation_input_tokens":200}),
                400,
                None,
                Some(200),
                None,
                None,
                true,
            ),
            (
                json!({"cache_read_input_tokens":600,"cache_creation_input_tokens":200}),
                1000,
                Some(600),
                Some(200),
                None,
                None,
                false,
            ),
            (
                json!({"cache_creation":{"ephemeral_5m_input_tokens":50}}),
                250,
                None,
                None,
                Some(50),
                None,
                true,
            ),
            (
                json!({"cache_creation":{"ephemeral_5m_input_tokens":50,"ephemeral_1h_input_tokens":100}}),
                350,
                None,
                None,
                Some(50),
                Some(100),
                true,
            ),
        ] {
            let mut receipt = json!({"input_tokens":200,"output_tokens":100});
            receipt
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let usage = parse_inference_usage(&receipt).expect("partial usage must survive");
            assert_eq!(
                usage,
                InferenceTokenUsage {
                    input_tokens_incomplete: incomplete,
                    input_tokens: input,
                    output_tokens: 100,
                    cache_read_tokens: read,
                    cache_write_tokens: write,
                    cache_write_5m_tokens: short,
                    cache_write_1h_tokens: long
                }
            );
            assert!(
                [
                    usage.cache_read_tokens,
                    usage.cache_write_tokens,
                    usage.cache_write_5m_tokens,
                    usage.cache_write_1h_tokens,
                ]
                .iter()
                .any(Option::is_none)
            );
            let cost = price_usage(usage);
            assert!(!cost.complete);
            assert_eq!(cost.unpriced_attempts, 1);
        }
    }
    fn price_usage(usage: InferenceTokenUsage) -> rara_observability::InferenceCostReport {
        let task = rara_observability::InferenceTask::default();
        let agent = task.start_agent(None);
        let call = agent.start_call(rara_observability::InferencePurpose::Main);
        let attempt = call.context().start_attempt("Anthropic", "fixture-model");
        attempt.record_final_usage(usage);
        let result: Result<(), ()> = Ok(());
        attempt.finish(&result);
        call.finish(&result);
        drop(agent);
        let table = rara_observability::InferencePriceTable {
            revision: "fixture".into(),
            prices: vec![rara_observability::InferencePrice {
                provider: "Anthropic".into(),
                model: "fixture-model".into(),
                input: 1.0,
                output: 1.0,
                cache_read: 1.0,
                cache_write: 1.0,
                cache_write_5m: 1.0,
                cache_write_1h: 1.0,
            }],
        };
        let snapshot = task.snapshot();
        assert_eq!(snapshot.attempts[0].usage, Some(usage));
        table.cost(&snapshot)
    }
}
