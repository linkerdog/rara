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
    use rara_agent::{
        Continuation, ContinuationContext, IterationBudget, LoopEffects, LoopEnd, LoopMachine,
        LoopProgress, ModelObservation, ModelRequest, ModelTurnEvent, ModelTurnPolicy,
        StopHookContext, StopHookOutcome, ToolBatchOutcome, ToolCall, execute_loop,
        execute_model_turn,
    };
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
            if messages.len() > 1 {
                let results = messages.last().expect("nonempty transcript");
                assert_eq!(results.role, "user");
                let blocks = results.content.as_array().expect("tool results");
                assert_eq!(blocks.len(), 2);
                assert_eq!(blocks[0]["tool_use_id"], "first");
                assert_eq!(blocks[1]["tool_use_id"], "second");
                on_event(LlmStreamEvent::TextDelta("done".into()));
                return Ok(LlmResponse {
                    content: vec![ContentBlock::Text {
                        text: "done".into(),
                    }],
                    stop_reason: Some("end_turn".into()),
                    usage: None,
                });
            }
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

    struct HostModelEvents<'a>(&'a mut Vec<String>);

    impl ModelTurnPolicy for HostModelEvents<'_> {
        fn event(&mut self, event: ModelTurnEvent) {
            match event {
                ModelTurnEvent::Stream(LlmStreamEvent::TextDelta(text))
                | ModelTurnEvent::AssistantText(text) => self.0.push(text),
                ModelTurnEvent::Stream(LlmStreamEvent::ReasoningDelta(_))
                | ModelTurnEvent::ToolUse(_) => {}
            }
        }
    }

    struct HostEffects {
        backend: Arc<dyn LlmBackend>,
        tools: ToolManager,
        transcript: Vec<Message>,
        assistant: Option<Message>,
        calls: Vec<ToolCall>,
        results: Vec<Value>,
        deltas: Vec<String>,
        tool_progress: Vec<ToolProgressEvent>,
        cancellation: Arc<AtomicBool>,
        finalized: Option<LoopEnd>,
    }

    impl HostEffects {
        fn new() -> Self {
            let mut tools = ToolManager::new();
            tools.register(Box::new(HostTool));
            Self {
                backend: Arc::new(HostBackend),
                tools,
                transcript: vec![Message {
                    role: "user".into(),
                    content: json!("echo twice"),
                }],
                assistant: None,
                calls: Vec::new(),
                results: Vec::new(),
                deltas: Vec::new(),
                tool_progress: Vec::new(),
                cancellation: Arc::new(AtomicBool::new(false)),
                finalized: None,
            }
        }
    }

    #[async_trait]
    impl LoopEffects for HostEffects {
        fn budget(&self) -> IterationBudget {
            IterationBudget::default()
        }

        async fn request_model(&mut self, _: LoopProgress) -> Result<ModelObservation> {
            let tools = self.tools.get_schemas();
            let request = ModelRequest {
                messages: &self.transcript,
                tools: &tools,
                metadata: LlmTurnMetadata::execute().with_cancellation(self.cancellation.clone()),
            };
            let output = execute_model_turn(
                self.backend.as_ref(),
                &request,
                &mut HostModelEvents(&mut self.deltas),
            )
            .await?;
            self.assistant = output.assistant_message;
            self.calls = output.tool_calls;
            Ok(ModelObservation {
                tool_call_count: self.calls.len(),
                response: output.response,
                ..Default::default()
            })
        }

        async fn record_assistant(&mut self, _: LoopProgress) -> Result<ContinuationContext> {
            if let Some(message) = self.assistant.take() {
                self.transcript.push(message);
            }
            Ok(ContinuationContext::default())
        }

        async fn continue_turn(
            &mut self,
            continuation: Continuation,
            _: LoopProgress,
        ) -> Result<()> {
            bail!("fixture did not request continuation: {continuation:?}")
        }

        async fn run_stop_hooks(
            &mut self,
            context: StopHookContext,
            _: LoopProgress,
        ) -> Result<StopHookOutcome> {
            assert!(!context.stop_hook_active);
            Ok(StopHookOutcome::AllowCompletion)
        }

        async fn run_tools(&mut self, _: LoopProgress) -> Result<ToolBatchOutcome> {
            for ToolCall { id, name, input } in std::mem::take(&mut self.calls) {
                let tool = self
                    .tools
                    .get_tool(&name)
                    .ok_or_else(|| anyhow::anyhow!("missing tool"))?;
                let context = ToolCallContext::default()
                    .with_session_id("host-session")
                    .with_turn_id("host-turn")
                    .with_call_id(id)
                    .with_cancellation(self.cancellation.clone());
                self.results.push(
                    tool.call_with_context_events(input, context, &mut |event| {
                        self.tool_progress.push(event)
                    })
                    .await?,
                );
            }
            Ok(ToolBatchOutcome::ResultsAvailable)
        }

        async fn commit_tool_results(&mut self, _: LoopProgress) -> Result<()> {
            self.transcript.push(Message {
                role: "user".into(),
                content: json!(
                    self.results
                        .iter()
                        .map(|result| json!({
                            "type": "tool_result", "tool_use_id": result["call"],
                            "content": result.to_string(), "is_error": false,
                        }))
                        .collect::<Vec<_>>()
                ),
            });
            Ok(())
        }

        async fn finalize(&mut self, end: LoopEnd, _: LoopProgress) -> Result<()> {
            self.finalized = Some(end);
            Ok(())
        }
    }

    #[test]
    fn shared_executor_preserves_deltas_identity_and_transcript_order() -> Result<()> {
        let mut host = HostEffects::new();
        let mut progress = LoopProgress::default();
        assert_eq!(
            immediate(execute_loop(&mut host, &mut progress))?,
            LoopEnd::ResponseComplete
        );
        assert_eq!(host.finalized, Some(LoopEnd::ResponseComplete));
        assert_eq!(progress.agentic_turns, 1);
        assert_eq!(host.deltas, ["two ", "calls", "done"]);
        assert_eq!(
            host.results,
            [
                json!({"text": "first", "session": "host-session", "turn": "host-turn", "call": "first"}),
                json!({"text": "second", "session": "host-session", "turn": "host-turn", "call": "second"}),
            ]
        );
        assert_eq!(
            host.tool_progress,
            vec![
                ToolProgressEvent::Output {
                    stream: ToolOutputStream::Stdout,
                    chunk: "echoing".into()
                };
                2
            ]
        );
        assert_eq!(
            host.transcript
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            ["user", "assistant", "user", "assistant"]
        );
        assert_eq!(host.transcript[3].content[0]["text"], "done");
        Ok(())
    }

    #[test]
    fn cancellation_from_the_host_prevents_execution_and_finalization() {
        let mut host = HostEffects::new();
        host.cancellation.store(true, Ordering::SeqCst);
        let mut progress = LoopProgress::default();
        assert!(immediate(execute_loop(&mut host, &mut progress)).is_err());
        assert!(host.results.is_empty());
        assert!(host.deltas.is_empty());
        assert_eq!(host.transcript.len(), 1);
        assert_eq!(host.finalized, None);
        assert_eq!(progress, LoopProgress::default());
    }

    #[test]
    fn pure_control_snapshot_retains_its_next_transition() -> Result<()> {
        let mut original = LoopMachine::new(LoopProgress::default());
        let model = original.begin_iteration(IterationBudget::default())?;
        let mut restored: LoopMachine = serde_json::from_slice(&serde_json::to_vec(&original)?)?;
        assert_eq!(
            original.model_completed(model.id, ModelObservation::default())?,
            restored.model_completed(model.id, ModelObservation::default())?
        );
        assert_eq!(original, restored);
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
