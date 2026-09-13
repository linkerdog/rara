//! Provider-boundary contract: the completion-model trait and its metadata.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use rara_observability::{InferenceAttempt, InferenceCallContext};
use serde_json::Value;

use super::contracts::{
    ContextBudget, LlmExecutionMode, LlmStreamEvent, ModelRequestFingerprint, ProviderCacheProfile,
};
use super::types::{LlmResponse, Message};

/// Per-turn metadata passed alongside a completion request.
#[derive(Debug, Clone)]
pub struct LlmTurnMetadata {
    execution_mode: LlmExecutionMode,
    cancellation: Option<Arc<AtomicBool>>,
    inference: Option<InferenceCallContext>,
}

impl Default for LlmTurnMetadata {
    fn default() -> Self {
        Self {
            execution_mode: LlmExecutionMode::Execute,
            cancellation: None,
            inference: None,
        }
    }
}

impl LlmTurnMetadata {
    pub fn execute() -> Self {
        Self {
            execution_mode: LlmExecutionMode::Execute,
            cancellation: None,
            inference: None,
        }
    }

    pub fn plan() -> Self {
        Self {
            execution_mode: LlmExecutionMode::Plan,
            cancellation: None,
            inference: None,
        }
    }

    pub fn with_cancellation(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn with_inference(mut self, inference: InferenceCallContext) -> Self {
        self.inference = Some(inference);
        self
    }

    pub fn inference(&self) -> Option<InferenceCallContext> {
        self.inference.clone()
    }

    pub fn execution_mode(&self) -> LlmExecutionMode {
        self.execution_mode
    }

    pub fn with_execution_mode(mut self, mode: LlmExecutionMode) -> Self {
        self.execution_mode = mode;
        self
    }

    pub fn start_attempt(&self, provider: &str, model: &str) -> Option<InferenceAttempt> {
        self.inference
            .as_ref()
            .map(|context| context.start_attempt(provider, model))
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|cancellation| cancellation.load(Ordering::SeqCst))
    }

    pub fn ensure_not_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(anyhow!("LLM turn cancelled by user"));
        }
        Ok(())
    }

    pub fn prefers_strong_reasoning(&self) -> bool {
        matches!(self.execution_mode, LlmExecutionMode::Plan)
    }
}

/// Exact main-request context retained after success, without accounting handles.
///
/// The first message is the generated system prompt; the rest is projected history.
#[derive(Clone, Debug)]
pub struct SummaryPrefix {
    pub messages: Vec<Message>,
    pub tools: Vec<Value>,
    pub execution_mode: LlmExecutionMode,
}

impl SummaryPrefix {
    pub fn matches_history(&self, messages: &[Message]) -> bool {
        let Some((system, history)) = self.messages.split_first() else {
            return false;
        };
        system.role == "system"
            && messages.len() >= history.len()
            && history.iter().zip(messages).all(|(old, new)| old == new)
    }

    pub fn messages_for_summary(
        &self,
        messages: &[Message],
        instruction: &str,
    ) -> Result<Vec<Message>> {
        if !self.matches_history(messages) {
            bail!("summary input does not share the captured main-request prefix");
        }
        let mut request = self.messages[..1].to_vec();
        request.extend_from_slice(messages);
        request.push(Message {
            role: "user".into(),
            content: Value::String(format!(
                "{instruction}\n\nReturn only the summary text. Do not call tools."
            )),
        });
        Ok(request)
    }
}

/// Provider boundary used by the agent loop and embedding hosts.
///
/// Implementors must preserve message and tool ordering. Backends that support
/// cooperative cancellation should override a context-aware request method
/// and observe `LlmTurnMetadata`; the default adapters cannot interrupt a
/// blocking `ask` implementation.
#[async_trait]
pub trait LlmBackend: Send + Sync {
    fn model_label(&self) -> Option<String> {
        None
    }

    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse>;
    async fn ask_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        _metadata: LlmTurnMetadata,
    ) -> Result<LlmResponse> {
        self.ask(messages, tools).await
    }

    async fn ask_streaming(
        &self,
        messages: &[Message],
        tools: &[Value],
        _on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask(messages, tools).await
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        _metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.ask_streaming(messages, tools, on_event).await
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String>;

    /// Summarize within the originating task's accounting and cancellation scope.
    async fn summarize_with_context(
        &self,
        messages: &[Message],
        instruction: &str,
        _metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.summarize(messages, instruction).await
    }

    /// Reuse a captured main prefix when supported. Compatibility backends keep
    /// their existing summary route; attempt identities reveal the actual model.
    async fn summarize_with_prefix(
        &self,
        messages: &[Message],
        instruction: &str,
        _prefix: &SummaryPrefix,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.summarize_with_context(messages, instruction, metadata)
            .await
    }

    /// Side-channel classifier call (auto-permission, background task status).
    ///
    /// Default implementation prepends an instructions message and delegates to
    /// `summarize()` via the auxiliary model, but backends may override to use a
    /// dedicated classifier endpoint or model.
    async fn classify(&self, instructions: &str, messages: &[Message]) -> Result<String> {
        let mut classify_msgs = vec![Message {
            role: "system".into(),
            content: Value::String(instructions.into()),
        }];
        classify_msgs.extend(messages.iter().cloned());
        self.summarize(&classify_msgs, instructions).await
    }

    /// Compatibility backends retain their classifier override and report missing
    /// attempt coverage until they implement this method.
    async fn classify_with_context(
        &self,
        instructions: &str,
        messages: &[Message],
        _metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.classify(instructions, messages).await
    }

    fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
        None
    }

    fn cache_profile(&self) -> ProviderCacheProfile {
        ProviderCacheProfile::none()
    }

    /// Return content-free hashes of the exact logical request sent by this backend.
    ///
    /// Implementors must never place raw prompt, tool, credential, or response
    /// content in the returned value.
    fn request_cache_fingerprint(
        &self,
        _messages: &[Message],
        _tools: &[Value],
        _metadata: &LlmTurnMetadata,
    ) -> Option<ModelRequestFingerprint> {
        None
    }
}
