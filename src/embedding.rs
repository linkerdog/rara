use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::Message;
use crate::llm::{
    ContextBudget, LlmBackend, LlmResponse, LlmStreamEvent, LlmTurnMetadata, ProviderCacheProfile,
};
use crate::local_model_server::{LocalModelServerState, LocalModelServerStatus};

const EMBEDDING_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddingInputKind {
    Document,
}

impl EmbeddingInputKind {
    fn as_request_value(self) -> &'static str {
        match self {
            Self::Document => "document",
        }
    }
}

#[async_trait]
pub(crate) trait EmbeddingBackend: Send + Sync {
    async fn embed(&self, text: &str, kind: EmbeddingInputKind) -> Result<Vec<f32>>;
}

pub(crate) struct LocalModelEmbeddingBackend {
    client: reqwest::Client,
    endpoint: String,
    backend: String,
}

impl LocalModelEmbeddingBackend {
    pub(crate) fn from_status(status: &LocalModelServerStatus) -> Option<Self> {
        if status.state != LocalModelServerState::Ready {
            return None;
        }
        let endpoint = status.endpoint.clone()?;
        let client = reqwest::Client::builder()
            .timeout(EMBEDDING_REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self {
            client,
            endpoint,
            backend: status.backend.clone(),
        })
    }
}

#[async_trait]
impl EmbeddingBackend for LocalModelEmbeddingBackend {
    async fn embed(&self, text: &str, kind: EmbeddingInputKind) -> Result<Vec<f32>> {
        let response = self
            .client
            .post(format!("{}/v1/embeddings", self.endpoint))
            .json(&json!({
                "input": text,
                "input_type": kind.as_request_value(),
                "backend": self.backend,
            }))
            .send()
            .await
            .context("send local embedding request")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("local embedding request failed with {status}: {body}");
        }
        let payload: EmbeddingResponse = response
            .json()
            .await
            .context("decode local embedding response")?;
        let item = payload
            .data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("local embedding response returned no vectors"))?;
        if item.embedding.is_empty() {
            bail!("local embedding response returned an empty vector");
        }
        Ok(item.embedding)
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
}

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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use anyhow::Result;

    use super::*;
    use crate::llm::{ContentBlock, LlmResponse};

    struct StaticChatBackend;

    #[async_trait]
    impl LlmBackend for StaticChatBackend {
        async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "chat".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: None,
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

    #[tokio::test]
    async fn local_model_embedding_backend_calls_openai_compatible_endpoint() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test server");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let request_body = Arc::new(Mutex::new(String::new()));
        let captured = request_body.clone();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = Vec::new();
            let mut buf = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buf).expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
                if let Some((headers, body)) = split_http_request(&request) {
                    if body.len() >= content_length(headers) {
                        *captured.lock().expect("lock body") =
                            String::from_utf8_lossy(body).to_string();
                        break;
                    }
                }
            }
            let body = r#"{"object":"list","model":"test","backend":"mlx_qwen3","data":[{"object":"embedding","index":0,"embedding":[0.25,0.5]}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        let backend = LocalModelEmbeddingBackend {
            client: reqwest::Client::new(),
            endpoint,
            backend: "mlx_qwen3".to_string(),
        };

        let embedding = backend
            .embed("hello", EmbeddingInputKind::Document)
            .await
            .expect("embedding response");

        assert_eq!(embedding, vec![0.25, 0.5]);
        let body = request_body.lock().expect("lock body").clone();
        let value: Value = serde_json::from_str(&body).expect("request json");
        assert_eq!(value["input"], "hello");
        assert_eq!(value["input_type"], "document");
        assert_eq!(value["backend"], "mlx_qwen3");
    }

    fn split_http_request(request: &[u8]) -> Option<(&[u8], &[u8])> {
        let marker = b"\r\n\r\n";
        let split = request
            .windows(marker.len())
            .position(|window| window == marker)?;
        Some((&request[..split], &request[split + marker.len()..]))
    }

    fn content_length(headers: &[u8]) -> usize {
        let headers = String::from_utf8_lossy(headers);
        headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or_default()
    }
}
