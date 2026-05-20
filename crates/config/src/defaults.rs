pub const DEFAULT_CODEX_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.4";
pub const DEFAULT_CODEX_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const DEFAULT_OPENAI_COMPATIBLE_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_OPENAI_COMPATIBLE_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
pub const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-chat";
pub const DEFAULT_KIMI_BASE_URL: &str = "https://api.moonshot.cn/v1";
pub const DEFAULT_KIMI_MODEL: &str = "kimi-k2.6";
pub const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_OPENROUTER_MODEL: &str = "openai/gpt-4o-mini";
pub const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_REASONING_SUMMARY: &str = "auto";
pub const REASONING_SUMMARY_NONE: &str = "none";
pub const REASONING_SUMMARY_DETAILED: &str = "detailed";
pub const LEGACY_CODEX_BASE_URL: &str = "http://localhost:8080";
pub const LEGACY_CODEX_MODEL: &str = "codex";
pub const LEGACY_CODEX_MODEL_V1: &str = "gpt-5-codex";
pub const LEGACY_CODEX_MODEL_V1_MINI: &str = "gpt-5-codex-mini";

// ── Memory consolidation (dream) defaults ────────────────────────
pub const DEFAULT_CONSOLIDATION_MODEL: &str = "inherit";
pub const DEFAULT_CONSOLIDATION_REASONING_EFFORT: &str = "low";
pub const DEFAULT_CONSOLIDATION_MIN_HOURS: u64 = 24;
pub const DEFAULT_CONSOLIDATION_MIN_SESSIONS: u64 = 5;
pub const DEFAULT_CONSOLIDATION_SCAN_INTERVAL_MINUTES: u64 = 10;

pub fn should_reset_codex_base_url(url: Option<&str>) -> bool {
    url.map(str::trim)
        .is_none_or(|value| value.is_empty() || value == LEGACY_CODEX_BASE_URL)
}

pub fn should_reset_codex_model(model: Option<&str>) -> bool {
    model.map(str::trim).is_none_or(|value| {
        value.is_empty()
            || matches!(
                value,
                LEGACY_CODEX_MODEL | LEGACY_CODEX_MODEL_V1 | LEGACY_CODEX_MODEL_V1_MINI
            )
    })
}

pub fn should_apply_codex_base_url(url: Option<&str>, expected: &str) -> bool {
    url.map(str::trim).is_none_or(|value| {
        value.is_empty()
            || value == LEGACY_CODEX_BASE_URL
            || ((value == DEFAULT_CODEX_BASE_URL || value == DEFAULT_CODEX_CHATGPT_BASE_URL)
                && value != expected)
    })
}
