use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::agent::Message;
use crate::llm::{
    ContextBudget, EmbeddingBackend, EmbeddingInputKind, LlmBackend, LlmResponse, LlmStreamEvent,
    LlmTurnMetadata, ProviderCacheProfile,
};

pub(crate) struct EmbeddingOverrideBackend {
    chat: Arc<dyn LlmBackend>,
    embeddings: Arc<dyn EmbeddingBackend>,
}

impl EmbeddingOverrideBackend {
    pub(crate) fn new(chat: Arc<dyn LlmBackend>, embeddings: Arc<dyn EmbeddingBackend>) -> Self {
        Self { chat, embeddings }
    }
}

#[async_trait]
impl LlmBackend for EmbeddingOverrideBackend {
    fn model_label(&self) -> Option<String> {
        self.chat.model_label()
    }

    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        self.chat.ask(messages, tools).await
    }

    async fn ask_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
    ) -> Result<LlmResponse> {
        self.chat.ask_with_context(messages, tools, metadata).await
    }

    async fn ask_streaming(
        &self,
        messages: &[Message],
        tools: &[Value],
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.chat.ask_streaming(messages, tools, on_event).await
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.chat
            .ask_streaming_with_context(messages, tools, metadata, on_event)
            .await
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.embeddings
            .embed(text, EmbeddingInputKind::Document)
            .await
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        self.chat.summarize(messages, instruction).await
    }

    async fn classify(&self, instructions: &str, messages: &[Message]) -> Result<String> {
        self.chat.classify(instructions, messages).await
    }

    fn context_budget(&self, messages: &[Message], tools: &[Value]) -> Option<ContextBudget> {
        self.chat.context_budget(messages, tools)
    }

    fn cache_profile(&self) -> ProviderCacheProfile {
        self.chat.cache_profile()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

    struct StaticChatBackend;

    #[async_trait]
    impl LlmBackend for StaticChatBackend {
        async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "chat".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..TokenUsage::default()
                }),
            })
        }

        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![9.0])
        }

        async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
            Ok("summary".to_string())
        }
    }

    struct StaticEmbeddingBackend;

    #[async_trait]
    impl EmbeddingBackend for StaticEmbeddingBackend {
        async fn embed(&self, text: &str, kind: EmbeddingInputKind) -> Result<Vec<f32>> {
            assert_eq!(text, "memory text");
            assert_eq!(kind, EmbeddingInputKind::Document);
            Ok(vec![1.0, 2.0, 3.0])
        }
    }

    #[tokio::test]
    async fn override_backend_delegates_chat_and_replaces_embedding() {
        let backend = EmbeddingOverrideBackend::new(
            Arc::new(StaticChatBackend),
            Arc::new(StaticEmbeddingBackend),
        );

        let chat = backend
            .ask(
                &[Message {
                    role: "user".to_string(),
                    content: Value::String("hi".to_string()),
                }],
                &[],
            )
            .await
            .expect("chat response");
        let embedding = backend.embed("memory text").await.expect("embedding");

        assert!(matches!(
            chat.content.as_slice(),
            [ContentBlock::Text { text }] if text == "chat"
        ));
        assert_eq!(embedding, vec![1.0, 2.0, 3.0]);
    }
}
