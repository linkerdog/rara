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
        ContinuationContext, IterationBudget, LoopEffect, LoopEnd, LoopMachine, LoopProgress,
        ModelObservation, ResponseEvidence, StopHookOutcome, ToolBatchOutcome,
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

    #[test]
    fn host_contracts_preserve_deltas_and_trusted_call_identity() -> Result<()> {
        let backend: Arc<dyn LlmBackend> = Arc::new(HostBackend);
        let mut tools = ToolManager::new();
        tools.register(Box::new(HostTool));
        let mut deltas = Vec::new();
        let mut machine = LoopMachine::new(LoopProgress::default());
        let model = machine.begin_iteration(IterationBudget::default())?;
        assert_eq!(model.effect, LoopEffect::RequestModel);
        let mut transcript = vec![Message {
            role: "user".into(),
            content: json!("echo twice"),
        }];
        let response = immediate(backend.ask_streaming_with_context(
            &transcript,
            &tools.get_schemas(),
            LlmTurnMetadata::execute(),
            &mut |event| {
                if let LlmStreamEvent::TextDelta(text) = event {
                    deltas.push(text);
                }
            },
        ))?;
        assert_eq!(deltas, ["two ", "calls"]);
        let assistant = machine.model_completed(
            model.id,
            ModelObservation {
                tool_call_count: response.content.len(),
                ..Default::default()
            },
        )?;
        assert_eq!(assistant.effect, LoopEffect::RecordAssistant);
        transcript.push(Message {
            role: "assistant".into(),
            content: serde_json::to_value(&response.content)?,
        });
        let tool_request =
            machine.assistant_recorded(assistant.id, ContinuationContext::default())?;
        assert_eq!(tool_request.effect, LoopEffect::RunTools);
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
        let commit =
            machine.tools_completed(tool_request.id, ToolBatchOutcome::ResultsAvailable)?;
        assert_eq!(commit.effect, LoopEffect::CommitToolResults);
        transcript.push(Message {
            role: "user".into(),
            content: json!(
                results
                    .iter()
                    .map(|result| json!({
                        "type": "tool_result", "tool_use_id": result["call"],
                        "content": result.to_string(), "is_error": false,
                    }))
                    .collect::<Vec<_>>()
            ),
        });
        // Only control state is restored; the host retains transcript/results.
        machine = serde_json::from_slice(&serde_json::to_vec(&machine)?)?;
        let model = machine.checkpoint_completed(commit.id, IterationBudget::default())?;
        assert_eq!(model.effect, LoopEffect::RequestModel);
        let response = immediate(backend.ask_streaming_with_context(
            &transcript,
            &tools.get_schemas(),
            LlmTurnMetadata::execute(),
            &mut |event| {
                if let LlmStreamEvent::TextDelta(text) = event {
                    deltas.push(text);
                }
            },
        ))?;
        let assistant = machine.model_completed(
            model.id,
            ModelObservation {
                response: ResponseEvidence {
                    had_text_response: true,
                    had_reasoning_response: false,
                },
                ..Default::default()
            },
        )?;
        transcript.push(Message {
            role: "assistant".into(),
            content: serde_json::to_value(response.content)?,
        });
        let hooks = machine.assistant_recorded(assistant.id, ContinuationContext::default())?;
        assert_eq!(
            hooks.effect,
            LoopEffect::RunStopHooks {
                stop_hook_active: false
            }
        );
        let finish = machine.stop_hooks_completed(hooks.id, StopHookOutcome::AllowCompletion)?;
        assert_eq!(
            machine.finalization_completed(finish.id)?,
            LoopEnd::ResponseComplete
        );
        assert_eq!(machine.progress().agentic_turns, 1);
        assert_eq!(deltas, ["two ", "calls", "done"]);
        assert_eq!(
            transcript.last().expect("final response").content[0]["text"],
            "done"
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
