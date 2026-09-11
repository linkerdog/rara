use aws_sdk_bedrockruntime::types::{
    CachePointBlock, CachePointType, CacheTtl, ContentBlock, Message, SystemContentBlock,
};

#[derive(Clone, Copy, Debug, Default)]
pub enum BedrockCacheTtl {
    #[default]
    FiveMinutes,
    OneHour,
}

pub fn supports_claude_cache(model: &str) -> bool {
    supports_cache_ttl(model, BedrockCacheTtl::FiveMinutes)
}

pub(crate) fn supports_cache_ttl(model: &str, ttl: BedrockCacheTtl) -> bool {
    let model = ["us.", "eu.", "apac.", "global."]
        .iter()
        .find_map(|region| model.strip_prefix(region))
        .unwrap_or(model);
    let families: &[&str] = match ttl {
        BedrockCacheTtl::FiveMinutes => &[
            "anthropic.claude-sonnet-4-5",
            "anthropic.claude-sonnet-4-6",
            "anthropic.claude-opus-4-20250514",
            "anthropic.claude-opus-4-5",
            "anthropic.claude-opus-4-6",
            "anthropic.claude-haiku-4-5",
            "anthropic.claude-3-7-sonnet",
            "anthropic.claude-3-5-sonnet-20241022-v2",
        ],
        // Bedrock's documented TTL support is narrower than the direct API.
        BedrockCacheTtl::OneHour => &[
            "anthropic.claude-sonnet-4-5",
            "anthropic.claude-opus-4-5",
            "anthropic.claude-haiku-4-5",
        ],
    };
    families.iter().any(|family| {
        model.strip_prefix(family).is_some_and(|suffix| {
            suffix.is_empty() || suffix.starts_with('-') || suffix.starts_with(':')
        })
    })
}

pub(crate) fn checkpoints(
    system: &mut Vec<SystemContentBlock>,
    messages: &mut [Message],
    ttl: BedrockCacheTtl,
) {
    let ttl = match ttl {
        BedrockCacheTtl::FiveMinutes => CacheTtl::FiveMinutes,
        BedrockCacheTtl::OneHour => CacheTtl::OneHour,
    };
    let point = CachePointBlock::builder()
        .r#type(CachePointType::Default)
        .ttl(ttl)
        .build()
        .expect("valid cache point");
    // The first system block is the runtime's complete stable instructions.
    // Later system controls are deliberately beyond the static checkpoint.
    if !system.is_empty() {
        system.insert(1, SystemContentBlock::CachePoint(point.clone()));
    }
    if let Some(last) = messages.last_mut() {
        last.content.push(ContentBlock::CachePoint(point));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_ttl_requires_a_verified_model_instead_of_a_family_guess() {
        for model in [
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "global.anthropic.claude-opus-4-5-20251101-v1:0",
            "anthropic.claude-haiku-4-5-20251001-v1:0",
        ] {
            assert!(supports_cache_ttl(model, BedrockCacheTtl::OneHour));
        }
        for model in [
            "anthropic.claude-opus-4-20250514-v1:0",
            "us.anthropic.claude-sonnet-4-6",
            "anthropic.claude-opus-4-6-v1",
            "anthropic.claude-opus-5",
            "arn:aws:bedrock:custom-model",
        ] {
            assert!(!supports_cache_ttl(model, BedrockCacheTtl::OneHour));
        }
        assert!(!supports_claude_cache("anthropic.claude-opus-5"));
    }

    #[test]
    fn checkpoints_keep_static_rules_separate_from_later_controls() {
        let mut system = vec![
            SystemContentBlock::Text("stable".into()),
            SystemContentBlock::Text("later control".into()),
        ];
        let mut messages = vec![
            Message::builder()
                .role(aws_sdk_bedrockruntime::types::ConversationRole::User)
                .content(ContentBlock::Text("task".into()))
                .build()
                .unwrap(),
        ];
        checkpoints(&mut system, &mut messages, BedrockCacheTtl::FiveMinutes);
        assert!(matches!(system[1], SystemContentBlock::CachePoint(_)));
        assert!(matches!(system[2], SystemContentBlock::Text(_)));
        assert!(matches!(
            messages[0].content[1],
            ContentBlock::CachePoint(_)
        ));
        assert!(supports_claude_cache("us.anthropic.claude-sonnet-4-6"));
        assert!(!supports_claude_cache("arn:aws:bedrock:custom-model"));
        assert!(!supports_claude_cache("amazon.nova-pro-v1:0"));
    }
}
