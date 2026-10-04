#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#![allow(clippy::expect_used)] // Browser fixture assertions and synchronous Promise construction.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use async_trait::async_trait;
use rara_agent::*;
use rara_core::llm::backend::{LlmBackend, LlmStreamCallback, LlmTurnMetadata};
use rara_core::llm::contracts::LlmStreamEvent;
use rara_core::llm::types::{ContentBlock, LlmResponse, Message};
use rara_core::tool::{
    Tool, ToolCallContext, ToolError, ToolOutputStream, ToolProgressCallback, ToolProgressEvent,
};
use serde_json::{Value, json};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

type Trace = Rc<RefCell<Vec<&'static str>>>;

async fn yield_to_browser() -> Result<()> {
    JsFuture::from(js_sys::Promise::resolve(&js_sys::JsString::from("ready")))
        .await
        .map_err(|error| anyhow::anyhow!("promise failed: {error:?}"))?;
    Ok(())
}

#[derive(Clone)]
struct Gate {
    promise: js_sys::Promise,
    resolve: js_sys::Function,
}

impl Gate {
    fn new() -> Self {
        let mut resolve = None;
        let promise = js_sys::Promise::new(&mut |callback, _| resolve = Some(callback));
        Self {
            promise,
            resolve: resolve.expect("synchronous promise resolver"),
        }
    }

    fn open(&self) {
        self.resolve
            .call0(&js_sys::JsString::from("ready"))
            .expect("resolve gate");
    }

    async fn wait(&self) {
        JsFuture::from(self.promise.clone())
            .await
            .expect("gate resolution");
    }
}

struct LocalBackend {
    trace: Trace,
    calls: Rc<Cell<usize>>,
}

#[async_trait(?Send)]
impl LlmBackend for LocalBackend {
    async fn ask(&self, _: &[Message], _: &[Value]) -> Result<LlmResponse> {
        anyhow::bail!("streaming path required")
    }

    async fn ask_streaming(
        &self,
        messages: &[Message],
        _: &[Value],
        on_event: &mut LlmStreamCallback<'async_trait>,
    ) -> Result<LlmResponse> {
        self.trace.borrow_mut().push("model-enter");
        let calls = self.calls.clone();
        yield_to_browser().await?;
        let first = calls.get() == 0;
        calls.set(calls.get() + 1);
        let content = if first {
            vec![ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "echo".into(),
                input: json!({"value": 7}),
            }]
        } else {
            assert_eq!(messages[1].content[0]["tool_use_id"], "call-1");
            assert_eq!(messages[1].content[0]["content"], r#"{"value":7}"#);
            on_event(LlmStreamEvent::TextDelta("done".into()));
            vec![ContentBlock::Text {
                text: "done".into(),
            }]
        };
        self.trace.borrow_mut().push("model-return");
        Ok(LlmResponse {
            content,
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }

    async fn summarize(&self, _: &[Message], _: &str) -> Result<String> {
        anyhow::bail!("summary not requested")
    }
}

struct Policy(Trace);

impl ModelTurnPolicy for Policy {
    fn event(&mut self, event: ModelTurnEvent) {
        self.0.borrow_mut().push(match event {
            ModelTurnEvent::Stream(_) => "model-stream",
            ModelTurnEvent::AssistantText(_) => "assistant-text",
            ModelTurnEvent::ToolUse(_) => "tool-use",
        });
    }
}

struct LocalTool {
    trace: Trace,
    gates: Option<(Gate, Gate)>,
}

#[async_trait(?Send)]
impl Tool for LocalTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Browser-owned echo"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _: Value) -> Result<Value, ToolError> {
        Err(ToolError::ExecutionFailed("context path required".into()))
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        report: &mut ToolProgressCallback<'async_trait>,
    ) -> Result<Value, ToolError> {
        assert_eq!(context.call_id(), Some("call-1"));
        self.trace.borrow_mut().push("tool-enter");
        report(ToolProgressEvent::Output {
            stream: ToolOutputStream::Stdout,
            chunk: "progress".into(),
        });
        if let Some((entered, release)) = &self.gates {
            entered.open();
            release.wait().await;
        } else {
            yield_to_browser()
                .await
                .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        }
        self.trace.borrow_mut().push("tool-cleanup");
        if context.is_cancelled() {
            return Err(ToolError::ExecutionFailed("cancelled after cleanup".into()));
        }
        Ok(input)
    }
}

struct Host {
    trace: Trace,
    backend: LocalBackend,
    tool: LocalTool,
    cancellation: Arc<AtomicBool>,
    pause: bool,
    output: Option<ModelTurnOutput>,
    history: Vec<Message>,
}

impl Host {
    fn new() -> Self {
        let trace = Trace::default();
        Self {
            backend: LocalBackend {
                trace: trace.clone(),
                calls: Rc::new(Cell::new(0)),
            },
            tool: LocalTool {
                trace: trace.clone(),
                gates: None,
            },
            trace,
            cancellation: Arc::new(AtomicBool::new(false)),
            pause: false,
            output: None,
            history: Vec::new(),
        }
    }
}

#[async_trait(?Send)]
impl ToolBatchEffects for Host {
    async fn prepare_call(&mut self, _: &ToolCall) -> Result<ToolAdmission> {
        self.trace.borrow_mut().push("admission");
        yield_to_browser().await?;
        Ok(if self.pause {
            ToolAdmission::AwaitingApproval
        } else {
            ToolAdmission::Invoke
        })
    }

    async fn invoke_call(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        let trace = self.trace.clone();
        execute_tool_call(
            &self.tool,
            call,
            ToolCallContext::default().with_cancellation(self.cancellation.clone()),
            &mut |progress| {
                assert_eq!(progress.call_id, "call-1");
                assert_eq!(progress.name, "echo");
                trace.borrow_mut().push("tool-progress");
            },
        )
        .await
    }

    async fn complete_call(
        &mut self,
        _: &ToolCall,
        result: Result<Value, ToolError>,
    ) -> Result<ToolReply> {
        self.trace.borrow_mut().push("tool-result");
        Ok(ToolReply::success(serde_json::to_string(&result?)?))
    }
}

#[async_trait(?Send)]
impl LoopEffects for Host {
    fn budget(&self) -> IterationBudget {
        IterationBudget {
            max_turns: Some(3),
            ..Default::default()
        }
    }

    async fn request_model(&mut self, _: LoopProgress) -> Result<ModelObservation> {
        let output = execute_model_turn(
            &self.backend,
            &ModelRequest {
                messages: &self.history,
                tools: &[],
                metadata: LlmTurnMetadata::execute().with_cancellation(self.cancellation.clone()),
            },
            &mut Policy(self.trace.clone()),
        )
        .await?;
        let observation = ModelObservation {
            tool_call_count: output.tool_calls.len(),
            response: output.response,
            ..Default::default()
        };
        self.output = Some(output);
        Ok(observation)
    }

    async fn record_assistant(&mut self, _: LoopProgress) -> Result<ContinuationContext> {
        if let Some(message) = self
            .output
            .as_mut()
            .and_then(|output| output.assistant_message.take())
        {
            self.history.push(message);
        }
        Ok(ContinuationContext::default())
    }

    async fn continue_turn(&mut self, _: Continuation, _: LoopProgress) -> Result<()> {
        anyhow::bail!("continuation not expected")
    }

    async fn run_stop_hooks(
        &mut self,
        _: StopHookContext,
        _: LoopProgress,
    ) -> Result<StopHookOutcome> {
        Ok(StopHookOutcome::AllowCompletion)
    }

    async fn run_tools(&mut self, _: LoopProgress) -> Result<ToolBatchOutcome> {
        let calls = std::mem::take(&mut self.output.as_mut().expect("model output").tool_calls);
        let output = execute_tool_batch(calls, self).await?;
        self.history.extend(output.messages);
        Ok(output.outcome)
    }

    async fn commit_tool_results(&mut self, _: LoopProgress) -> Result<()> {
        self.trace.borrow_mut().push("commit");
        yield_to_browser().await
    }

    async fn finalize(&mut self, _: LoopEnd, _: LoopProgress) -> Result<()> {
        yield_to_browser().await?;
        self.trace.borrow_mut().push("finalize");
        Ok(())
    }
}

#[wasm_bindgen_test]
async fn browser_executes_local_model_tool_and_loop_effects_in_order() {
    let mut host = Host::new();
    let end = execute_loop(&mut host, &mut LoopProgress::default())
        .await
        .expect("browser loop");
    assert_eq!(end, LoopEnd::ResponseComplete);
    assert_eq!(
        *host.trace.borrow(),
        [
            "model-enter",
            "model-return",
            "tool-use",
            "admission",
            "tool-enter",
            "tool-progress",
            "tool-cleanup",
            "tool-result",
            "commit",
            "model-enter",
            "model-stream",
            "model-return",
            "finalize"
        ]
    );
    assert_eq!(host.history.len(), 3);
}

#[wasm_bindgen_test]
async fn browser_approval_pause_never_invokes_the_tool() {
    let mut host = Host::new();
    host.pause = true;
    let end = execute_loop(&mut host, &mut LoopProgress::default())
        .await
        .expect("approval pause");
    assert_eq!(end, LoopEnd::AwaitingApproval);
    assert_eq!(
        *host.trace.borrow(),
        [
            "model-enter",
            "model-return",
            "tool-use",
            "admission",
            "finalize"
        ]
    );
}

#[wasm_bindgen_test]
async fn browser_cancel_waits_for_pending_tool_cleanup() {
    let mut host = Host::new();
    let entered = Gate::new();
    let release = Gate::new();
    host.tool.gates = Some((entered.clone(), release.clone()));
    let cancellation = host.cancellation.clone();
    let trace = host.trace.clone();
    let completed = Gate::new();
    let done = completed.clone();
    let result = Rc::new(RefCell::new(None));
    let returned = result.clone();
    spawn_local(async move {
        let outcome = execute_loop(&mut host, &mut LoopProgress::default()).await;
        *returned.borrow_mut() = Some(outcome);
        done.open();
    });
    entered.wait().await;
    cancellation.store(true, Ordering::SeqCst);
    yield_to_browser().await.expect("browser task tick");
    assert!(result.borrow().is_none());
    assert!(!trace.borrow().contains(&"tool-cleanup"));
    assert!(!trace.borrow().contains(&"finalize"));
    release.open();
    completed.wait().await;
    assert!(result.borrow_mut().take().expect("loop returned").is_err());
    assert_eq!(
        *trace.borrow(),
        [
            "model-enter",
            "model-return",
            "tool-use",
            "admission",
            "tool-enter",
            "tool-progress",
            "tool-cleanup",
            "tool-result"
        ]
    );
}
