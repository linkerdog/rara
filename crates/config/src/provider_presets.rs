pub struct ProviderPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub env: &'static str,
}

pub const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        env: "OPENAI_API_KEY",
    },
    ProviderPreset {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        env: "GROQ_API_KEY",
    },
    ProviderPreset {
        id: "together",
        name: "Together AI",
        base_url: "https://api.together.xyz/v1",
        env: "TOGETHER_API_KEY",
    },
    ProviderPreset {
        id: "xai",
        name: "xAI",
        base_url: "https://api.x.ai/v1",
        env: "XAI_API_KEY",
    },
    ProviderPreset {
        id: "mistral",
        name: "Mistral",
        base_url: "https://api.mistral.ai/v1",
        env: "MISTRAL_API_KEY",
    },
    ProviderPreset {
        id: "minimax",
        name: "MiniMax",
        base_url: "https://api.minimax.io/v1",
        env: "MINIMAX_API_KEY",
    },
    ProviderPreset {
        id: "zai",
        name: "Z.ai",
        base_url: "https://api.z.ai/api/paas/v4",
        env: "ZAI_API_KEY",
    },
    ProviderPreset {
        id: "zai-coding-plan",
        name: "Z.ai Coding Plan",
        base_url: "https://api.z.ai/api/coding/paas/v4",
        env: "ZAI_API_KEY",
    },
    ProviderPreset {
        id: "hyperbolic",
        name: "Hyperbolic",
        base_url: "https://api.hyperbolic.xyz/v1",
        env: "HYPERBOLIC_API_KEY",
    },
    ProviderPreset {
        id: "moonshotai",
        name: "Moonshot AI",
        base_url: "https://api.moonshot.ai/v1",
        env: "MOONSHOT_API_KEY",
    },
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        env: "DEEPSEEK_API_KEY",
    },
    ProviderPreset {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        env: "OPENROUTER_API_KEY",
    },
];

pub fn provider_preset(id: &str) -> Option<&'static ProviderPreset> {
    PROVIDER_PRESETS.iter().find(|preset| preset.id == id)
}
