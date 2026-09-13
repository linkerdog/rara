use std::fmt::Write as _;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use super::ProviderCacheProfile;
use crate::config::OpenAiEndpointKind;

/// Explicit retention for supported Anthropic routes. Continuous tool loops
/// should use the short TTL; longer retention needs a known human pause pattern.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnthropicCacheTtl {
    #[default]
    FiveMinutes,
    OneHour,
}

impl AnthropicCacheTtl {
    fn control(self) -> Value {
        match self {
            Self::FiveMinutes => json!({"type": "ephemeral", "ttl": "5m"}),
            Self::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
        }
    }
}

fn official_endpoint(base_url: &str, host: &str, paths: &[&str]) -> bool {
    Url::parse(base_url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some(host)
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && paths.contains(&url.path().trim_end_matches('/'))
    })
}

pub(super) fn chat_billing_provider(base_url: &str, kind: OpenAiEndpointKind) -> String {
    let official = match kind {
        OpenAiEndpointKind::Custom => official_endpoint(base_url, "api.openai.com", &["", "/v1"]),
        OpenAiEndpointKind::Deepseek => {
            official_endpoint(base_url, "api.deepseek.com", &["", "/v1"])
        }
        OpenAiEndpointKind::Openrouter => {
            official_endpoint(base_url, "openrouter.ai", &["/api", "/api/v1"])
        }
        OpenAiEndpointKind::Kimi => {
            official_endpoint(base_url, "api.moonshot.ai", &["", "/v1"])
                || official_endpoint(base_url, "api.moonshot.cn", &["", "/v1"])
        }
        OpenAiEndpointKind::KimiCoding => {
            official_endpoint(base_url, "api.kimi.com", &["/coding", "/coding/v1"])
        }
    };
    if official {
        if kind == OpenAiEndpointKind::Custom {
            "OpenAI".into()
        } else {
            kind.label().into()
        }
    } else {
        endpoint_billing_id(kind.label(), base_url)
    }
}

pub(super) fn responses_billing_provider(base_url: &str) -> String {
    if official_endpoint(base_url, "api.openai.com", &["/v1"]) {
        "OpenAI".into()
    } else if official_endpoint(base_url, "chatgpt.com", &["/backend-api/codex"]) {
        "ChatGPT Codex".into()
    } else {
        endpoint_billing_id("Responses", base_url)
    }
}

fn endpoint_billing_id(label: &str, base_url: &str) -> String {
    // Custom gateways can charge different rates for the same model ID.
    // Keep them distinct without disclosing private endpoint paths or credentials.
    let mut identity = format!("{label}:");
    for byte in Sha256::digest(base_url.trim_end_matches('/').as_bytes()) {
        write!(&mut identity, "{byte:02x}").expect("writing to String cannot fail");
    }
    identity
}

fn model_family(model: &str, families: &[&str]) -> bool {
    families.iter().any(|family| {
        model == *family
            || model
                .strip_prefix(family)
                .is_some_and(|suffix| suffix.starts_with('-') || suffix.starts_with('.'))
    })
}

fn openai_cached_model(model: &str) -> bool {
    model_family(
        model,
        &["gpt-4o", "gpt-4.1", "gpt-5", "gpt-6", "o1", "o3", "o4-mini"],
    )
}

pub(super) fn openrouter_anthropic(base_url: &str, kind: OpenAiEndpointKind, model: &str) -> bool {
    kind == OpenAiEndpointKind::Openrouter
        && official_endpoint(base_url, "openrouter.ai", &["/api", "/api/v1"])
        && model_family(
            model,
            &[
                "anthropic/claude-sonnet-4",
                "anthropic/claude-opus-4",
                "anthropic/claude-haiku-4.5",
                "anthropic/claude-sonnet-5",
                "anthropic/claude-opus-5",
            ],
        )
}

pub(super) fn chat_cache_profile(
    base_url: &str,
    kind: OpenAiEndpointKind,
    model: &str,
) -> ProviderCacheProfile {
    let automatic = match kind {
        OpenAiEndpointKind::Deepseek => {
            official_endpoint(base_url, "api.deepseek.com", &["", "/v1"])
                && model_family(
                    model,
                    &[
                        "deepseek-chat",
                        "deepseek-reasoner",
                        "deepseek-v4",
                        "deepseek-flash",
                    ],
                )
        }
        OpenAiEndpointKind::Custom => {
            official_endpoint(base_url, "api.openai.com", &["", "/v1"])
                && openai_cached_model(model)
        }
        OpenAiEndpointKind::Openrouter => {
            official_endpoint(base_url, "openrouter.ai", &["/api", "/api/v1"])
                && (model
                    .strip_prefix("openai/")
                    .is_some_and(openai_cached_model)
                    || model_family(
                        model,
                        &[
                            "deepseek/deepseek-chat",
                            "deepseek/deepseek-r1",
                            "deepseek/deepseek-v3",
                            "deepseek/deepseek-v4",
                        ],
                    ))
        }
        // Code Assist and coding subscriptions do not inherit API billing/cache
        // contracts from similarly named commercial models.
        OpenAiEndpointKind::Kimi | OpenAiEndpointKind::KimiCoding => false,
    };
    if automatic {
        ProviderCacheProfile::automatic_prefix_cache_with_usage()
    } else if openrouter_anthropic(base_url, kind, model) {
        ProviderCacheProfile {
            explicit_prefix_cache: true,
            cache_usage_accounting: true,
            cache_retention_control: true,
            ..ProviderCacheProfile::none()
        }
    } else {
        ProviderCacheProfile::none()
    }
}

pub(super) fn responses_cache_profile(base_url: &str, model: &str) -> ProviderCacheProfile {
    if openai_cached_model(model)
        && (official_endpoint(base_url, "api.openai.com", &["/v1"])
            || official_endpoint(base_url, "chatgpt.com", &["/backend-api/codex"]))
    {
        ProviderCacheProfile::automatic_prefix_cache_with_usage()
    } else {
        ProviderCacheProfile::none()
    }
}

/// Set a static system checkpoint and an advancing conversation checkpoint.
/// Explicit block controls work across OpenRouter's supported Anthropic routes;
/// top-level automatic controls would constrain routing to Anthropic direct.
pub(super) fn apply_anthropic_breakpoints(body: &mut Value, ttl: AnthropicCacheTtl) {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    let static_index = messages
        .iter()
        .take_while(|message| message["role"] == "system")
        .count()
        .checked_sub(1);
    let advancing_index = messages.iter().rposition(|message| {
        matches!(
            message["role"].as_str(),
            Some("user" | "assistant" | "tool")
        ) && message.get("content").is_some_and(|content| {
            content.as_str().is_some_and(|text| !text.is_empty())
                || content
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "text"))
        })
    });
    for index in [static_index, advancing_index].into_iter().flatten() {
        let content = &mut messages[index]["content"];
        if let Some(text) = content.as_str() {
            *content = json!([{"type": "text", "text": text}]);
        }
        if let Some(blocks) = content.as_array_mut()
            && let Some(block) = blocks
                .iter_mut()
                .rev()
                .find(|block| block["type"] == "text")
        {
            block["cache_control"] = ttl.control();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_capabilities_require_endpoint_and_model_identity() {
        let auto = |url, kind, model| chat_cache_profile(url, kind, model).automatic_prefix_cache;
        assert!(auto(
            "https://api.deepseek.com",
            OpenAiEndpointKind::Deepseek,
            "deepseek-v4-flash"
        ));
        assert!(!auto(
            "https://gateway.example/v1",
            OpenAiEndpointKind::Deepseek,
            "deepseek-v4-flash"
        ));
        assert!(auto(
            "https://api.deepseek.com",
            OpenAiEndpointKind::Deepseek,
            "deepseek-flash"
        ));
        assert!(auto(
            "https://api.openai.com/v1",
            OpenAiEndpointKind::Custom,
            "gpt-5.4"
        ));
        for endpoint in [
            "https://api.openai.com.evil.test/v1",
            "http://api.openai.com/v1",
            "https://api.openai.com/v1/proxy",
        ] {
            assert!(!auto(endpoint, OpenAiEndpointKind::Custom, "gpt-5.4"));
        }
        assert!(!auto(
            "https://api.openai.com/v1",
            OpenAiEndpointKind::Custom,
            "gpt-50"
        ));
        assert!(!auto(
            "https://openrouter.ai/api/v1",
            OpenAiEndpointKind::Openrouter,
            "openrouter/auto"
        ));
        assert!(
            responses_cache_profile("https://chatgpt.com/backend-api/codex", "gpt-5.4")
                .automatic_prefix_cache
        );
        assert!(
            !responses_cache_profile("https://custom.example/v1", "gpt-5.4").automatic_prefix_cache
        );
    }

    #[test]
    fn billing_identity_separates_gateways_and_subscription_routes() {
        let first = chat_billing_provider(
            "https://gateway-a.example/secret-path",
            OpenAiEndpointKind::Deepseek,
        );
        let second = chat_billing_provider(
            "https://gateway-b.example/secret-path",
            OpenAiEndpointKind::Deepseek,
        );
        assert_ne!(first, second);
        assert_ne!(first, "DeepSeek");
        assert!(!first.contains("secret-path"));
        assert_eq!(
            chat_billing_provider("https://api.deepseek.com", OpenAiEndpointKind::Deepseek),
            "DeepSeek"
        );
        assert_ne!(
            responses_billing_provider("https://chatgpt.com/backend-api/codex"),
            responses_billing_provider("https://api.openai.com/v1")
        );
    }

    #[test]
    fn anthropic_static_checkpoint_stays_fixed_and_conversation_checkpoint_advances() {
        let initial = json!({"messages": [
            {"role": "system", "content": "stable rules"},
            {"role": "user", "content": "first task"}
        ]});
        let mut first = initial.clone();
        apply_anthropic_breakpoints(&mut first, AnthropicCacheTtl::FiveMinutes);
        let mut next = initial;
        next["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role": "user", "content": "second task"}));
        apply_anthropic_breakpoints(&mut next, AnthropicCacheTtl::FiveMinutes);
        assert_eq!(first["messages"][0], next["messages"][0]);
        assert_eq!(
            next["messages"][0]["content"][0]["cache_control"]["ttl"],
            "5m"
        );
        assert!(next["messages"][1]["content"].is_string());
        assert_eq!(
            next["messages"][2]["content"][0]["cache_control"]["ttl"],
            "5m"
        );
        assert!(next.get("cache_control").is_none());
        let mut long = json!({"messages": [{"role": "system", "content": "stable"}]});
        apply_anthropic_breakpoints(&mut long, AnthropicCacheTtl::OneHour);
        assert_eq!(
            long["messages"][0]["content"][0]["cache_control"]["ttl"],
            "1h"
        );
    }
}
