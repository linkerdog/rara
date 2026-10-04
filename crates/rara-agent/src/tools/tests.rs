use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::{Context, Poll, Waker};

use rara_core::tool::ToolOutputStream;

use super::*;

#[derive(Default)]
struct Effects {
    trace: Arc<Mutex<Vec<String>>>,
    released: Arc<AtomicUsize>,
    admissions: VecDeque<ToolAdmission>,
    fail_at: Option<usize>,
    tool_fails: bool,
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[test]
fn native_tool_batch_future_remains_send() {
    fn assert_send(_: impl Future + Send) {}
    assert_send(execute_tool_batch(Vec::new(), &mut Effects::default()));
}

impl Effects {
    async fn step(&mut self, label: String) -> Result<()> {
        let index = {
            let mut trace = self
                .trace
                .lock()
                .map_err(|e| anyhow::anyhow!("trace poisoned: {e}"))?;
            trace.push(label);
            trace.len()
        };
        poll_fn(|_| {
            if self.released.load(Ordering::SeqCst) >= index {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        if self.fail_at == Some(index) {
            return Err(std::io::Error::other("policy failed").into());
        }
        Ok(())
    }
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
impl ToolBatchEffects for Effects {
    async fn begin_batch(&mut self, _: &[ToolCall]) -> Result<()> {
        self.step("begin".into()).await
    }

    async fn prepare_call(&mut self, call: &ToolCall) -> Result<ToolAdmission> {
        self.step(format!("prepare:{}", call.id)).await?;
        Ok(self.admissions.pop_front().unwrap_or(ToolAdmission::Invoke))
    }

    async fn invoke_call(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        self.step(format!("invoke:{}", call.id))
            .await
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if self.tool_fails {
            return Err(ToolError::InvalidInput("rejected".into()));
        }
        Ok(json!({"call": call.id}))
    }

    async fn complete_call(
        &mut self,
        call: &ToolCall,
        result: Result<Value, ToolError>,
    ) -> Result<ToolReply> {
        self.step(format!("complete:{}", call.id)).await?;
        Ok(match result {
            Ok(value) => ToolReply::success(value.to_string()),
            Err(error) => ToolReply::error(format!("Error: {error}")),
        })
    }
}

fn calls() -> Vec<ToolCall> {
    ["first", "second"]
        .into_iter()
        .map(|id| ToolCall {
            id: id.into(),
            name: "echo".into(),
            input: json!({"call_id": "forged"}),
        })
        .collect()
}

fn immediate<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => result,
        Poll::Pending => anyhow::bail!("effect unexpectedly suspended"),
    }
}

#[test]
fn pending_admission_invocation_and_result_policy_block_later_calls() -> Result<()> {
    let mut effects = Effects::default();
    let trace = effects.trace.clone();
    let released = effects.released.clone();
    let mut future = pin!(execute_tool_batch(
        calls(),
        &mut effects as &mut dyn ToolBatchEffects
    ));
    let mut context = Context::from_waker(Waker::noop());
    let expected = [
        "begin",
        "prepare:first",
        "invoke:first",
        "complete:first",
        "prepare:second",
        "invoke:second",
        "complete:second",
    ];
    for index in 0..expected.len() {
        released.store(index, Ordering::SeqCst);
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            *trace
                .lock()
                .map_err(|e| anyhow::anyhow!("trace poisoned: {e}"))?,
            expected[..=index]
        );
    }
    released.store(expected.len(), Ordering::SeqCst);
    let Poll::Ready(result) = future.as_mut().poll(&mut context) else {
        anyhow::bail!("completion pending");
    };
    let output = result?;
    assert_eq!(output.outcome, ToolBatchOutcome::ResultsAvailable);
    assert_eq!(output.messages.len(), 2);
    for (message, id) in output.messages.iter().zip(["first", "second"]) {
        assert_eq!(message.role, "user");
        assert_eq!(message.content[0]["tool_use_id"], id);
        assert!(message.content[0].get("is_error").is_none());
    }
    Ok(())
}

#[test]
fn approval_preserves_earlier_replies_and_skips_paused_and_later_calls() -> Result<()> {
    let mut effects = Effects {
        admissions: VecDeque::from([
            ToolAdmission::Reply(ToolReply::success("handled")),
            ToolAdmission::AwaitingApproval,
        ]),
        ..Default::default()
    };
    effects.released.store(usize::MAX, Ordering::SeqCst);
    let mut calls = calls();
    calls.push(ToolCall {
        id: "later".into(),
        name: "echo".into(),
        input: json!({}),
    });
    let output = immediate(execute_tool_batch(calls, &mut effects))?;
    assert_eq!(output.outcome, ToolBatchOutcome::AwaitingApproval);
    assert_eq!(
        output.messages,
        [ToolReply::success("handled").into_message("first")]
    );
    assert_eq!(
        *effects
            .trace
            .lock()
            .map_err(|e| anyhow::anyhow!("trace poisoned: {e}"))?,
        ["begin", "prepare:first", "prepare:second"]
    );
    Ok(())
}

#[test]
fn omission_is_explicit_and_tool_errors_can_become_error_replies() -> Result<()> {
    let mut effects = Effects {
        admissions: VecDeque::from([ToolAdmission::Omit]),
        tool_fails: true,
        ..Default::default()
    };
    effects.released.store(usize::MAX, Ordering::SeqCst);
    let output = immediate(execute_tool_batch(calls(), &mut effects))?;
    assert_eq!(output.outcome, ToolBatchOutcome::ResultsAvailable);
    assert_eq!(
        output.messages,
        [ToolReply::error("Error: Invalid input: rejected").into_message("second")]
    );
    Ok(())
}

#[test]
fn policy_failures_stop_admission_without_replay_or_error_conversion() -> Result<()> {
    for index in [1, 2, 4] {
        let mut effects = Effects {
            fail_at: Some(index),
            ..Default::default()
        };
        effects.released.store(usize::MAX, Ordering::SeqCst);
        let error = immediate(execute_tool_batch(calls(), &mut effects))
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected policy error"))?;
        assert!(error.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(
            effects
                .trace
                .lock()
                .map_err(|e| anyhow::anyhow!("trace poisoned: {e}"))?
                .len(),
            index
        );
    }
    Ok(())
}

struct ProbeTool {
    release: Arc<AtomicBool>,
    returned: Arc<AtomicBool>,
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
impl Tool for ProbeTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Test trusted invocation"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _: Value) -> Result<Value, ToolError> {
        Err(ToolError::ExecutionFailed("context required".into()))
    }
    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        report: &mut rara_core::tool::ToolProgressCallback<'async_trait>,
    ) -> Result<Value, ToolError> {
        assert_eq!(context.session_id(), Some("session"));
        assert_eq!(context.turn_id(), Some("turn"));
        assert_eq!(context.call_id(), Some("first"));
        assert_eq!(
            context.workspace_root(),
            Some(std::path::Path::new("workspace"))
        );
        assert_eq!(input["call_id"], "forged");
        report(ToolProgressEvent::Output {
            stream: ToolOutputStream::Stdout,
            chunk: "started".into(),
        });
        poll_fn(|_| {
            if self.release.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        self.returned.store(true, Ordering::SeqCst);
        if context.is_cancelled() {
            return Err(ToolError::ExecutionFailed("cancelled after cleanup".into()));
        }
        Ok(json!({"trusted_call": context.call_id()}))
    }
}

#[test]
fn invocation_binds_identity_and_waits_for_cooperative_cancellation_cleanup() -> Result<()> {
    for cancel in [false, true] {
        let tool = ProbeTool {
            release: Arc::new(AtomicBool::new(false)),
            returned: Arc::new(AtomicBool::new(false)),
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let context = ToolCallContext::default()
            .with_session_id("session")
            .with_turn_id("turn")
            .with_workspace_root("workspace")
            .with_call_id("wrong-context-call")
            .with_cancellation(cancellation.clone());
        let mut events = Vec::new();
        let call = calls().remove(0);
        {
            let mut report = |event| events.push(event);
            let mut future = pin!(execute_tool_call(&tool, &call, context, &mut report));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(future.as_mut().poll(&mut cx).is_pending());
            cancellation.store(cancel, Ordering::SeqCst);
            assert!(future.as_mut().poll(&mut cx).is_pending());
            assert!(!tool.returned.load(Ordering::SeqCst));
            tool.release.store(true, Ordering::SeqCst);
            let Poll::Ready(result) = future.as_mut().poll(&mut cx) else {
                anyhow::bail!("tool did not return");
            };
            if cancel {
                assert!(result.is_err());
            } else {
                assert_eq!(result?, json!({"trusted_call": "first"}));
            }
        }
        assert!(tool.returned.load(Ordering::SeqCst));
        assert_eq!(
            events,
            [ToolCallProgress {
                call_id: "first".into(),
                name: "echo".into(),
                event: ToolProgressEvent::Output {
                    stream: ToolOutputStream::Stdout,
                    chunk: "started".into()
                }
            }]
        );
    }
    Ok(())
}
