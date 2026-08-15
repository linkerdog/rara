use super::ProviderFamily;
use crate::config::{OpenAiEndpointKind, RaraConfig};

pub const CODEX_MODEL_PRESETS: [(&str, &str, &str); 0] = [];

pub const OPENAI_COMPATIBLE_MODEL_PRESETS: [(&str, &str, &str); 5] = [
    ("Custom endpoint", "openai-compatible", "gpt-4o-mini"),
    ("DeepSeek", "openai-compatible", "deepseek-chat"),
    ("Moonshot AI", "openai-compatible", "kimi-k2.6"),
    ("Kimi For Coding", "openai-compatible", "kimi-for-coding"),
    ("OpenRouter", "openai-compatible", "openai/gpt-4o-mini"),
];

pub const KIMI_CODING_MODEL_PRESETS: [(&str, &str, &str); 1] =
    [("Kimi For Coding", "kimi-coding", "kimi-for-coding")];

pub const LOCAL_MODEL_PRESETS: [(&str, &str, &str); 3] = [
    ("Gemma 4 E4B (Experimental)", "gemma4", "gemma4-e4b"),
    ("Gemma 4 E2B (Experimental)", "gemma4", "gemma4-e2b"),
    ("Qwn3 8B", "qwn3", "qwn3-8b"),
];

pub const OLLAMA_MODEL_PRESETS: [(&str, &str, &str); 3] = [
    ("Gemma 4", "ollama", "gemma4"),
    ("Gemma 4 E4B", "ollama", "gemma4:e4b"),
    ("Gemma 4 E2B", "ollama", "gemma4:e2b"),
];

pub const BEDROCK_MODEL_PRESETS: [(&str, &str, &str); 3] = [
    (
        "Claude Sonnet 4",
        "bedrock",
        "us.anthropic.claude-sonnet-4-20250514-v1:0",
    ),
    (
        "Claude 3.5 Sonnet v2",
        "bedrock",
        "us.anthropic.claude-3-5-sonnet-20241022-v2:0",
    ),
    ("Nova Pro", "bedrock", "us.amazon.nova-pro-v1:0"),
];

pub fn selected_provider_family_idx_for_config(config: &RaraConfig) -> usize {
    let family = match config.provider.as_str() {
        "codex" => ProviderFamily::Codex,
        "deepseek" => ProviderFamily::DeepSeek,
        "openai-compatible" => {
            if config.active_openai_profile_kind() == Some(OpenAiEndpointKind::Deepseek) {
                ProviderFamily::DeepSeek
            } else if config.active_openai_profile_kind() == Some(OpenAiEndpointKind::Kimi) {
                ProviderFamily::Kimi
            } else if config.active_openai_profile_kind() == Some(OpenAiEndpointKind::KimiCoding) {
                ProviderFamily::KimiCoding
            } else {
                ProviderFamily::OpenAiCompatible
            }
        }
        "kimi" => ProviderFamily::Kimi,
        "kimi-coding" => ProviderFamily::KimiCoding,
        "openrouter" => ProviderFamily::OpenAiCompatible,
        "gemini" | "gemini-code-assist" => ProviderFamily::Gemini,
        "ollama" | "ollama-native" | "ollama-openai" => ProviderFamily::Ollama,
        "bedrock" => ProviderFamily::Bedrock,
        "gemma4" | "qwn3" | "qwen3" => ProviderFamily::CandleLocal,
        _ => ProviderFamily::Codex,
    };
    provider_family_index(family)
}

fn provider_family_index(family: ProviderFamily) -> usize {
    super::PROVIDER_FAMILIES
        .iter()
        .position(|(candidate, _, _)| *candidate == family)
        .unwrap_or(0)
}

pub fn current_model_presets(
    provider_picker_idx: usize,
) -> &'static [(&'static str, &'static str, &'static str)] {
    match super::PROVIDER_FAMILIES[provider_picker_idx].0 {
        ProviderFamily::Codex => &CODEX_MODEL_PRESETS,
        ProviderFamily::DeepSeek => &[],
        ProviderFamily::Kimi => &[],
        ProviderFamily::KimiCoding => &KIMI_CODING_MODEL_PRESETS,
        ProviderFamily::OpenAiCompatible => &OPENAI_COMPATIBLE_MODEL_PRESETS,
        ProviderFamily::Gemini => &[],
        ProviderFamily::CandleLocal => &LOCAL_MODEL_PRESETS,
        ProviderFamily::Ollama => &OLLAMA_MODEL_PRESETS,
        ProviderFamily::Bedrock => &BEDROCK_MODEL_PRESETS,
    }
}

pub fn selected_preset_idx_for_config(config: &RaraConfig, provider_picker_idx: usize) -> usize {
    if matches!(
        super::PROVIDER_FAMILIES[provider_picker_idx].0,
        ProviderFamily::OpenAiCompatible
    ) {
        let kind = config
            .active_openai_profile_kind()
            .unwrap_or(OpenAiEndpointKind::Custom);
        return openai_compatible_preset_index(kind);
    }
    current_model_presets(provider_picker_idx)
        .iter()
        .position(|(_, provider, model)| {
            config.provider == *provider && config.model.as_deref() == Some(*model)
        })
        .unwrap_or(0)
}

pub fn openai_compatible_preset_kind(idx: usize) -> OpenAiEndpointKind {
    match idx {
        1 => OpenAiEndpointKind::Deepseek,
        2 => OpenAiEndpointKind::Kimi,
        3 => OpenAiEndpointKind::KimiCoding,
        4 => OpenAiEndpointKind::Openrouter,
        _ => OpenAiEndpointKind::Custom,
    }
}

pub fn openai_compatible_preset_index(kind: OpenAiEndpointKind) -> usize {
    match kind {
        OpenAiEndpointKind::Custom => 0,
        OpenAiEndpointKind::Deepseek => 1,
        OpenAiEndpointKind::Kimi => 2,
        OpenAiEndpointKind::KimiCoding => 3,
        OpenAiEndpointKind::Openrouter => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        openai_compatible_preset_index, openai_compatible_preset_kind,
        selected_provider_family_idx_for_config,
    };
    use crate::config::{OpenAiEndpointKind, RaraConfig};

    #[test]
    fn keeps_generic_openai_compatible_provider_in_its_own_family() {
        let config = RaraConfig {
            provider: "openai-compatible".to_string(),
            ..RaraConfig::default()
        };

        assert_eq!(selected_provider_family_idx_for_config(&config), 4);
    }

    #[test]
    fn keeps_local_and_ollama_provider_families_stable() {
        let local = RaraConfig {
            provider: "gemma4".to_string(),
            ..RaraConfig::default()
        };
        let ollama = RaraConfig {
            provider: "ollama".to_string(),
            ..RaraConfig::default()
        };
        let ollama_native = RaraConfig {
            provider: "ollama-native".to_string(),
            ..RaraConfig::default()
        };
        let ollama_openai = RaraConfig {
            provider: "ollama-openai".to_string(),
            ..RaraConfig::default()
        };

        assert_eq!(selected_provider_family_idx_for_config(&local), 6);
        assert_eq!(selected_provider_family_idx_for_config(&ollama), 7);
        assert_eq!(selected_provider_family_idx_for_config(&ollama_native), 7);
        assert_eq!(selected_provider_family_idx_for_config(&ollama_openai), 7);
    }

    #[test]
    fn keeps_legacy_openai_endpoint_providers_in_openai_compatible_family() {
        let config = RaraConfig {
            provider: "openrouter".to_string(),
            ..RaraConfig::default()
        };
        assert_eq!(selected_provider_family_idx_for_config(&config), 4);
    }

    #[test]
    fn routes_kimi_provider_to_dedicated_family() {
        let config = RaraConfig {
            provider: "kimi".to_string(),
            ..RaraConfig::default()
        };

        assert_eq!(selected_provider_family_idx_for_config(&config), 3);
    }

    #[test]
    fn routes_kimi_coding_provider_to_dedicated_family() {
        let config = RaraConfig {
            provider: "kimi-coding".to_string(),
            ..RaraConfig::default()
        };

        assert_eq!(selected_provider_family_idx_for_config(&config), 2);
    }

    #[test]
    fn routes_deepseek_provider_to_dedicated_family() {
        let config = RaraConfig {
            provider: "deepseek".to_string(),
            ..RaraConfig::default()
        };

        assert_eq!(selected_provider_family_idx_for_config(&config), 1);
    }

    #[test]
    fn openai_preset_kind_roundtrips() {
        for kind in [
            OpenAiEndpointKind::Custom,
            OpenAiEndpointKind::Deepseek,
            OpenAiEndpointKind::Kimi,
            OpenAiEndpointKind::KimiCoding,
            OpenAiEndpointKind::Openrouter,
        ] {
            assert_eq!(
                openai_compatible_preset_kind(openai_compatible_preset_index(kind)),
                kind
            );
        }
    }
}
