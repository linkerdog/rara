mod bedrock;
mod cache_policy;
mod codex_tools_compat;
pub(crate) mod deepseek_anthropic;
pub(crate) mod deepseek_dsml;
mod gemini;
mod gemini_schema;
#[cfg(test)]
mod inference_tests;
mod inference_transport;

#[cfg(test)]
pub(crate) const MAX_CHAT_ATTEMPTS_PER_CALL: usize = {
    let stream_attempts = openai_compatible::STREAM_IDLE_RETRY_ATTEMPTS + 1;
    let model_attempts = if stream_attempts > 2 {
        stream_attempts
    } else {
        2
    };
    model_attempts * (inference_transport::MAX_SEND_RETRIES + 1)
};
#[cfg(test)]
mod model_context_tests;
mod ollama;
mod openai_compatible;
mod shared;
mod summary;
#[cfg(test)]
mod tests;
mod types;

pub use self::bedrock::BedrockBackend;
pub use self::cache_policy::AnthropicCacheTtl;
pub(crate) use self::deepseek_anthropic::wrap_if_eligible as wrap_deepseek_anthropic_if_eligible;
pub use self::gemini::GeminiBackend;
pub use self::ollama::OllamaBackend;
#[cfg(test)]
pub(crate) use self::openai_compatible::context_window_error_for_test;
pub(crate) use self::openai_compatible::infer_openai_compatible_auxiliary_model;
pub(crate) use self::openai_compatible::is_context_window_error;
pub use self::openai_compatible::{
    CodexBackend, OpenAiCompatibleBackend, fetch_model_context_window,
};
pub(crate) use self::shared::is_retryable_http_error;
pub use self::shared::{
    ContextBudget, LlmBackend, LlmExecutionMode, LlmStreamEvent, LlmTurnMetadata, MockLlm,
    ProviderCacheProfile,
};
pub use self::summary::{SummaryPrefix, SummaryStrategy};
pub use self::types::{ContentBlock, LlmResponse, Message, TokenUsage};
