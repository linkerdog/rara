use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use super::*;

fn complete_tool_call(future: impl Future<Output = Result<Value, ToolError>>) -> Result<Value> {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => Ok(result?),
        Poll::Pending => anyhow::bail!("fixture tool unexpectedly suspended"),
    }
}

struct TestTool {
    name: &'static str,
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[test]
fn native_tool_contracts_and_futures_remain_thread_safe() {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send(_: impl Future + Send) {}
    assert_send_sync::<Box<dyn Tool>>();
    assert_send_sync::<ToolManager>();
    assert_send(TestTool { name: "echo" }.call(Value::Null));
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
impl Tool for TestTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "test tool"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Ok(Value::Null)
    }
}

#[test]
fn schemas_are_returned_in_stable_name_order() {
    let mut manager = ToolManager::new();
    manager.register(Box::new(TestTool { name: "zeta_tool" }));
    manager.register(Box::new(TestTool { name: "alpha_tool" }));
    manager.register(Box::new(TestTool { name: "mid_tool" }));

    let schemas = manager.get_schemas();
    let names = schemas
        .iter()
        .map(|schema| schema["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["alpha_tool", "mid_tool", "zeta_tool"]);
}

#[test]
fn filtered_schemas_preserve_stable_name_order() {
    let mut manager = ToolManager::new();
    manager.register(Box::new(TestTool { name: "zeta_tool" }));
    manager.register(Box::new(TestTool { name: "alpha_tool" }));
    manager.register(Box::new(TestTool { name: "mid_tool" }));

    let schemas = manager.get_schemas_filtered(|name| name != "mid_tool");
    let names = schemas
        .iter()
        .map(|schema| schema["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["alpha_tool", "zeta_tool"]);
}

#[test]
fn call_context_retains_workspace_root() {
    let context = ToolCallContext::default()
        .with_session_id("session-1")
        .with_turn_id("turn-1")
        .with_call_id("call-1")
        .with_workspace_root("/tmp/rara-workspace");

    assert_eq!(context.session_id(), Some("session-1"));
    assert_eq!(context.turn_id(), Some("turn-1"));
    assert_eq!(context.call_id(), Some("call-1"));
    assert_eq!(
        context.workspace_root(),
        Some(Path::new("/tmp/rara-workspace"))
    );
}

struct EventTool;

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
impl Tool for EventTool {
    fn name(&self) -> &str {
        "alpha_tool"
    }

    fn description(&self) -> &str {
        "event-aware replacement"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({"type": "object"})
    }

    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Err(ToolError::ExecutionFailed("event path required".into()))
    }

    async fn call_with_events(
        &self,
        input: Value,
        report: &mut crate::tool::ToolProgressCallback<'async_trait>,
    ) -> Result<Value, ToolError> {
        report(ToolProgressEvent::Output {
            stream: ToolOutputStream::Stderr,
            chunk: "diagnostic".into(),
        });
        Ok(input)
    }
}

#[test]
fn context_default_preserves_event_aware_dispatch() -> Result<()> {
    let tool: &dyn Tool = &EventTool;
    let mut events = Vec::new();
    let input = serde_json::json!({"value": 42});
    let result = complete_tool_call(tool.call_with_context_events(
        input.clone(),
        ToolCallContext::default(),
        &mut |event| events.push(event),
    ))?;
    assert_eq!(result, input);
    assert_eq!(
        events,
        [ToolProgressEvent::Output {
            stream: ToolOutputStream::Stderr,
            chunk: "diagnostic".into(),
        }]
    );
    Ok(())
}

#[test]
fn context_default_preserves_call_only_dispatch() -> Result<()> {
    let tool: &dyn Tool = &TestTool {
        name: "simple_tool",
    };
    let mut events = Vec::new();
    let result = complete_tool_call(tool.call_with_context_events(
        Value::Null,
        ToolCallContext::default(),
        &mut |event| events.push(event),
    ))?;
    assert_eq!(result, Value::Null);
    assert!(events.is_empty());
    Ok(())
}

#[test]
fn duplicate_registration_replaces_implementation_and_retain_preserves_schemas() -> Result<()> {
    let mut manager = ToolManager::new();
    manager.register(Box::new(TestTool { name: "zeta_tool" }));
    manager.register(Box::new(TestTool { name: "alpha_tool" }));
    manager.register(Box::new(EventTool));
    let replacement = manager
        .get_tool("alpha_tool")
        .ok_or_else(|| anyhow::anyhow!("missing replacement"))?;
    assert_eq!(replacement.description(), "event-aware replacement");
    manager.retain(|name| name == "alpha_tool");
    assert!(manager.get_tool("zeta_tool").is_none());
    assert_eq!(
        manager.get_schemas(),
        [serde_json::json!({
            "name": "alpha_tool", "description": "event-aware replacement",
            "input_schema": {"type": "object"},
        })]
    );
    Ok(())
}
