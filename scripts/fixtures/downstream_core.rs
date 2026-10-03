//! Public-contract fixture, deliberately independent of the application crate.

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::task::{Context, Poll, Waker};

    use anyhow::{Result, bail};
    use async_trait::async_trait;
    use rara_core::llm::backend::{LlmBackend, LlmTurnMetadata};
    use rara_core::llm::contracts::LlmStreamEvent;
    use rara_core::llm::types::{ContentBlock, LlmResponse, Message};
    use rara_core::tool::{
        Tool, ToolCallContext, ToolError, ToolManager, ToolOutputStream, ToolProgressEvent,
    };
    use serde_json::{Value, json};

    // These fake implementations never suspend. Polling once checks their
    // dispatch contract without adding an executor to the dependency fixture.
    fn immediate<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result,
            Poll::Pending => bail!("fixture unexpectedly suspended"),
        }
    }

    struct HostTool;

    #[async_trait]
    impl Tool for HostTool {
        fn name(&self) -> &str {
            "host_echo"
        }
        fn description(&self) -> &str {
            "Echo with trusted host identity"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn call(&self, _input: Value) -> Result<Value, ToolError> {
            Err(ToolError::ExecutionFailed("host context required".into()))
        }
        async fn call_with_context_events(
            &self,
            input: Value,
            context: ToolCallContext,
            report: &mut (dyn FnMut(ToolProgressEvent) + Send),
        ) -> Result<Value, ToolError> {
            if context.is_cancelled() {
                return Err(ToolError::ExecutionFailed("cancelled".into()));
            }
            report(ToolProgressEvent::Output {
                stream: ToolOutputStream::Stdout,
                chunk: "echoing".into(),
            });
            Ok(json!({
                "text": input["text"], "session": context.session_id(),
                "turn": context.turn_id(), "call": context.call_id(),
            }))
        }
    }

    struct HostBackend;

    #[async_trait]
    impl LlmBackend for HostBackend {
        async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
            bail!("streaming context required")
        }
        async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
            bail!("summary not used by this fixture")
        }
        async fn ask_streaming_with_context(
            &self,
            messages: &[Message],
            tools: &[Value],
            metadata: LlmTurnMetadata,
            on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
        ) -> Result<LlmResponse> {
            metadata.ensure_not_cancelled()?;
            assert_eq!(
                messages,
                &[Message {
                    role: "user".into(),
                    content: json!("echo twice")
                }]
            );
            assert_eq!(
                tools,
                &[json!({
                    "name": "host_echo", "description": "Echo with trusted host identity",
                    "input_schema": {"type": "object", "properties": {"text": {"type": "string"}}},
                })]
            );
            on_event(LlmStreamEvent::TextDelta("two ".into()));
            on_event(LlmStreamEvent::TextDelta("calls".into()));
            Ok(LlmResponse {
                content: ["first", "second"]
                    .into_iter()
                    .map(|id| ContentBlock::ToolUse {
                        id: id.into(),
                        name: "host_echo".into(),
                        input: json!({"text": id, "session_id": "untrusted", "call_id": "forged"}),
                    })
                    .collect(),
                stop_reason: Some("tool_use".into()),
                usage: None,
            })
        }
    }

    #[test]
    fn host_contracts_preserve_deltas_and_trusted_call_identity() -> Result<()> {
        let backend: Arc<dyn LlmBackend> = Arc::new(HostBackend);
        let mut tools = ToolManager::new();
        tools.register(Box::new(HostTool));
        let mut deltas = Vec::new();
        let response = immediate(backend.ask_streaming_with_context(
            &[Message {
                role: "user".into(),
                content: json!("echo twice"),
            }],
            &tools.get_schemas(),
            LlmTurnMetadata::execute(),
            &mut |event| {
                if let LlmStreamEvent::TextDelta(text) = event {
                    deltas.push(text);
                }
            },
        ))?;
        assert_eq!(deltas, ["two ", "calls"]);
        let mut results = Vec::new();
        let mut progress = Vec::new();
        for block in response.content {
            let ContentBlock::ToolUse { id, name, input } = block else {
                bail!("expected a tool call");
            };
            let tool = tools
                .get_tool(&name)
                .ok_or_else(|| anyhow::anyhow!("missing tool"))?;
            let context = ToolCallContext::default()
                .with_session_id("host-session")
                .with_turn_id("host-turn")
                .with_call_id(id);
            results.push(immediate(async {
                Ok(tool
                    .call_with_context_events(input, context, &mut |event| progress.push(event))
                    .await?)
            })?);
        }
        assert_eq!(
            results,
            [
                json!({"text": "first", "session": "host-session", "turn": "host-turn", "call": "first"}),
                json!({"text": "second", "session": "host-session", "turn": "host-turn", "call": "second"}),
            ]
        );
        assert_eq!(
            progress,
            vec![
                ToolProgressEvent::Output {
                    stream: ToolOutputStream::Stdout,
                    chunk: "echoing".into(),
                };
                2
            ]
        );
        Ok(())
    }

    #[test]
    fn shared_cancellation_reaches_backend_and_tool_context() -> Result<()> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let metadata = LlmTurnMetadata::execute().with_cancellation(cancelled.clone());
        let context = ToolCallContext::default().with_cancellation(cancelled.clone());
        assert!(!metadata.is_cancelled());
        assert!(!context.is_cancelled());
        cancelled.store(true, Ordering::SeqCst);
        assert!(
            immediate(HostBackend.ask_streaming_with_context(&[], &[], metadata, &mut |_| {},))
                .is_err()
        );
        let mut progress = Vec::new();
        let result = immediate(async {
            Ok(HostTool
                .call_with_context_events(json!({"text": "ignored"}), context, &mut |event| {
                    progress.push(event)
                })
                .await?)
        });
        assert!(result.is_err());
        assert!(progress.is_empty());
        Ok(())
    }
}
