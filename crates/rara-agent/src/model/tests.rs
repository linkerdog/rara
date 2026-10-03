use std::future::Future;
use std::pin::pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use serde_json::json;

use super::*;

struct Backend {
    blocks: Vec<ContentBlock>,
    events: Vec<LlmStreamEvent>,
    called: AtomicBool,
    fail: bool,
}

#[async_trait]
impl LlmBackend for Backend {
    async fn ask(&self, _: &[Message], _: &[Value]) -> Result<LlmResponse> {
        anyhow::bail!("context-aware streaming required")
    }

    async fn summarize(&self, _: &[Message], _: &str) -> Result<String> {
        anyhow::bail!("summary not requested")
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.called.store(true, Ordering::SeqCst);
        assert_eq!(messages[0].content, json!("prepared prefix"));
        assert_eq!(tools, &[json!({"name": "echo"})]);
        assert!(metadata.prefers_strong_reasoning());
        for item in &self.events {
            event(item.clone());
        }
        if self.fail {
            return Err(std::io::Error::other("provider failed").into());
        }
        Ok(LlmResponse {
            content: self.blocks.clone(),
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }
}

fn backend(blocks: Vec<ContentBlock>) -> Backend {
    Backend {
        blocks,
        events: Vec::new(),
        called: AtomicBool::new(false),
        fail: false,
    }
}

#[derive(Default)]
struct Policy {
    events: Vec<ModelTurnEvent>,
    order: Vec<&'static str>,
    fail_text: bool,
}

impl ModelTurnPolicy for Policy {
    fn event(&mut self, event: ModelTurnEvent) {
        self.order.push(match &event {
            ModelTurnEvent::Stream(_) => "stream",
            ModelTurnEvent::AssistantText(_) => "text_event",
            ModelTurnEvent::ToolUse(_) => "tool_event",
        });
        self.events.push(event);
    }

    fn observe_response(&mut self, result: &Result<LlmResponse>) {
        self.order
            .push(if result.is_ok() { "response" } else { "error" });
    }

    fn prepare_text(&mut self, text: &str, stream: StreamEvidence) -> Result<String> {
        self.order.push("prepare_text");
        if self.fail_text {
            anyhow::bail!("text policy failed");
        }
        let text = text.replace("<private/>", "");
        if !stream.text_delta && !text.trim().is_empty() {
            self.event(ModelTurnEvent::AssistantText(text.clone()));
        }
        Ok(text)
    }

    fn prepare_tool_input(&mut self, call: &ToolCall) -> Result<Value> {
        self.order.push("prepare_tool");
        Ok(json!({"approved": call.input["raw"]}))
    }

    fn finish_response(&mut self) -> Result<()> {
        self.order.push("finish");
        Ok(())
    }
}

fn run(backend: &Backend, policy: &mut dyn ModelTurnPolicy) -> Result<ModelTurnOutput> {
    let messages = [Message {
        role: "system".into(),
        content: json!("prepared prefix"),
    }];
    let tools = [json!({"name": "echo"})];
    let request = ModelRequest {
        messages: &messages,
        tools: &tools,
        metadata: LlmTurnMetadata::plan(),
    };
    match pin!(execute_model_turn(backend, &request, policy))
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(result) => result,
        Poll::Pending => anyhow::bail!("immediate backend suspended"),
    }
}

#[test]
fn policies_preserve_provider_transcript_and_execution_identity_in_order() -> Result<()> {
    let backend = backend(vec![
        ContentBlock::ProviderMetadata {
            provider: "fake".into(),
            key: "reasoning_content".into(),
            value: json!("thought"),
        },
        ContentBlock::Text {
            text: "answer<private/>".into(),
        },
        ContentBlock::ToolUse {
            id: "provider-call".into(),
            name: "echo".into(),
            input: json!({"raw": 42}),
        },
    ]);
    let mut policy = Policy::default();
    let output = run(&backend, &mut policy)?;
    assert_eq!(
        policy.order,
        [
            "response",
            "prepare_text",
            "text_event",
            "prepare_tool",
            "tool_event",
            "finish"
        ]
    );
    let message = output
        .assistant_message
        .ok_or_else(|| anyhow::anyhow!("missing assistant"))?;
    assert_eq!(message.role, "assistant");
    assert_eq!(message.content[0]["value"], "thought");
    assert_eq!(message.content[1]["text"], "answer");
    assert_eq!(message.content[2]["input"], json!({"raw": 42}));
    assert_eq!(
        output.tool_calls,
        [ToolCall {
            id: "provider-call".into(),
            name: "echo".into(),
            input: json!({"approved": 42})
        }]
    );
    assert!(
        matches!(policy.events.last(), Some(ModelTurnEvent::ToolUse(call)) if call == &output.tool_calls[0])
    );
    assert_eq!(
        output.response,
        ResponseEvidence {
            had_text_response: true,
            had_reasoning_response: true
        }
    );
    assert_eq!(output.stop_reason.as_deref(), Some("end_turn"));
    Ok(())
}

#[test]
fn streamed_text_suppresses_fallback_and_reasoning_counts_without_metadata() -> Result<()> {
    let mut backend = backend(vec![ContentBlock::Text {
        text: "answer".into(),
    }]);
    backend.events = vec![
        LlmStreamEvent::ReasoningDelta("thought".into()),
        LlmStreamEvent::TextDelta("answer".into()),
    ];
    let mut policy = Policy::default();
    let output = run(&backend, &mut policy)?;
    assert_eq!(
        policy.order,
        ["stream", "stream", "response", "prepare_text", "finish"]
    );
    assert_eq!(policy.events.len(), 2);
    assert_eq!(
        output.stream,
        StreamEvidence {
            text_delta: true,
            reasoning_delta: true
        }
    );
    assert!(output.response.had_reasoning_response);
    Ok(())
}

#[test]
fn default_policy_emits_fallback_only_without_streamed_text() -> Result<()> {
    struct Events(Vec<ModelTurnEvent>);
    impl ModelTurnPolicy for Events {
        fn event(&mut self, event: ModelTurnEvent) {
            self.0.push(event);
        }
    }
    for streamed in [false, true] {
        let mut backend = backend(vec![ContentBlock::Text {
            text: "answer".into(),
        }]);
        if streamed {
            backend
                .events
                .push(LlmStreamEvent::TextDelta("answer".into()));
        }
        let mut policy = Events(Vec::new());
        let output = run(&backend, &mut policy)?;
        assert_eq!(policy.0.len(), 1);
        match &policy.0[0] {
            ModelTurnEvent::Stream(LlmStreamEvent::TextDelta(text)) => {
                assert!(streamed);
                assert_eq!(text, "answer");
            }
            ModelTurnEvent::AssistantText(text) => {
                assert!(!streamed);
                assert_eq!(text, "answer");
            }
            other => anyhow::bail!("unexpected event: {other:?}"),
        }
        assert!(output.response.had_text_response);
    }
    Ok(())
}

#[test]
fn metadata_and_sanitized_empty_text_do_not_create_assistant_history() -> Result<()> {
    let backend = backend(vec![
        ContentBlock::Text {
            text: "<private/> ".into(),
        },
        ContentBlock::ProviderMetadata {
            provider: "fake".into(),
            key: "reasoning_content".into(),
            value: json!("thought"),
        },
    ]);
    let output = run(&backend, &mut Policy::default())?;
    assert_eq!(output.assistant_message, None);
    assert!(output.response.is_reasoning_only());
    Ok(())
}

#[test]
fn provider_error_is_observed_and_preserved_without_collecting_blocks() -> Result<()> {
    let mut backend = backend(Vec::new());
    backend.fail = true;
    backend.events = vec![LlmStreamEvent::TextDelta("partial".into())];
    let mut policy = Policy::default();
    let error = run(&backend, &mut policy)
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected failure"))?;
    assert!(error.downcast_ref::<std::io::Error>().is_some());
    assert_eq!(policy.order, ["stream", "error"]);
    Ok(())
}

#[test]
fn cancelled_request_skips_backend_and_still_notifies_policy() {
    let backend = backend(Vec::new());
    let request = ModelRequest {
        messages: &[],
        tools: &[],
        metadata: LlmTurnMetadata::plan().with_cancellation(Arc::new(AtomicBool::new(true))),
    };
    let mut policy = Policy::default();
    assert!(matches!(
        pin!(execute_model_turn(&backend, &request, &mut policy))
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(_))
    ));
    assert!(!backend.called.load(Ordering::SeqCst));
    assert_eq!(policy.order, ["error"]);
}

#[test]
fn policy_error_prevents_later_tool_events_and_finalization() {
    let backend = backend(vec![
        ContentBlock::Text {
            text: "answer".into(),
        },
        ContentBlock::ToolUse {
            id: "unused".into(),
            name: "echo".into(),
            input: json!({}),
        },
    ]);
    let mut policy = Policy {
        fail_text: true,
        ..Default::default()
    };
    assert!(run(&backend, &mut policy).is_err());
    assert_eq!(policy.order, ["response", "prepare_text"]);
    assert!(policy.events.is_empty());
}
