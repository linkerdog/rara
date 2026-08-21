//! In-process hook runtime that subscribes to the RuntimeEventBus
//! and dispatches matching AgentEvent variants to registered hook callbacks.
//!
//! Hooks are declared through the control plane (HookControlRequest::Declare)
//! and can be registered at any time — before or after calling `start`.
//! The dispatch loop runs on a dedicated Tokio task and matches events
//! against the current set of registered hooks.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use crate::agent::AgentEvent;
use crate::runtime_control::{HookEvent, HookLifecycle, RuntimeEvent};
use crate::runtime_event_bus::RuntimeEventBus;

/// A registered in-process hook combining a lifecycle trigger and a callback.
type HookCallback = Box<dyn Fn(&AgentEvent) + Send + Sync>;

struct HookEntry {
    lifecycle: HookLifecycle,
    callback: HookCallback,
}

/// In-process hook dispatch runtime.
///
/// Hooks are stored behind an `Arc<RwLock<HashMap<...>>>` so that the
/// control-plane can register hooks while the dispatch loop is already running.
pub struct HookRuntime {
    bus: Arc<RuntimeEventBus>,
    hooks: Arc<RwLock<HashMap<String, HookEntry>>>,
    /// Collected stdout from command hooks. Drained before each model turn
    /// and injected as system messages into the model context.
    outputs: Arc<std::sync::Mutex<Vec<String>>>,
    started: AtomicBool,
}

impl HookRuntime {
    pub fn new(bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            bus,
            hooks: Arc::new(RwLock::new(HashMap::new())),
            outputs: Arc::new(std::sync::Mutex::new(Vec::new())),
            started: AtomicBool::new(false),
        }
    }

    /// Push a hook output (typically command stdout) into the collection buffer.
    pub fn push_output(&self, text: String) {
        if let Ok(mut guard) = self.outputs.lock() {
            guard.push(text);
        }
    }

    /// Publish structured plugin hook command output to control-plane
    /// subscribers without injecting it into model context.
    pub fn publish_plugin_hook_output(&self, event: HookEvent) -> usize {
        self.bus.publish_control(RuntimeEvent::Hook(event))
    }

    /// Drain outputs synchronously (for use in non-async contexts).
    pub fn blocking_drain_outputs(&self) -> Vec<String> {
        if let Ok(mut guard) = self.outputs.lock() {
            std::mem::take(&mut *guard)
        } else {
            Vec::new()
        }
    }

    pub fn modify_tool_input(
        &self,
        _tool_name: &str,
        input: serde_json::Value,
    ) -> serde_json::Value {
        input
    }

    /// Register an in-process hook.  Safe to call while `start` is running.
    pub fn register(&self, hook_id: String, lifecycle: HookLifecycle, callback: HookCallback) {
        self.hooks
            .write()
            .expect("hook runtime registry lock poisoned")
            .insert(
                hook_id,
                HookEntry {
                    lifecycle,
                    callback,
                },
            );
    }

    /// Return the number of registered hooks.
    pub fn hook_count(&self) -> usize {
        self.hooks
            .read()
            .expect("hook runtime registry lock poisoned")
            .len()
    }

    pub fn dispatch_memory_query(&self, query: &str) {
        let event = AgentEvent::MemoryAction {
            message: format!("MemoryQuery: {query}"),
        };
        let guard = self
            .hooks
            .read()
            .expect("hook runtime registry lock poisoned");
        for entry in guard.values() {
            if entry.lifecycle == HookLifecycle::MemoryQuery {
                (entry.callback)(&event);
            }
        }
    }

    /// Start the hook dispatch loop on a dedicated Tokio task.
    ///
    /// Returns immediately if already started (idempotent).
    /// The returned handle can be aborted to stop hook processing.
    /// Callers may safely call `register` / `unregister` after this returns.
    pub fn start(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.started.swap(true, Ordering::SeqCst) {
            return None;
        }
        let hooks = Arc::clone(&self.hooks);
        let mut rx = self.bus.subscribe();

        let handle = tokio::task::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let guard = hooks.read().expect("hook runtime registry lock poisoned");
                        for entry in guard.values() {
                            if lifecycle_matches(&entry.lifecycle, &event) {
                                (entry.callback)(&event);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
        Some(handle)
    }
}

/// Map an AgentEvent variant to the HookLifecycle it corresponds to.
///
/// `AgentStart` deliberately returns `None` because it fires once per agent
/// turn, not once per session.  The session-level lifecycle is managed by the
/// session control plane, not by per-turn agent events.
fn lifecycle_for_event(event: &AgentEvent) -> Option<HookLifecycle> {
    match event {
        AgentEvent::AgentStart => None,
        AgentEvent::AgentStop { .. } => Some(HookLifecycle::Stop),
        AgentEvent::ToolUse { .. } => Some(HookLifecycle::PreToolUse),
        AgentEvent::ToolResult { .. } => Some(HookLifecycle::PostToolUse),
        AgentEvent::MemoryAction { .. } => Some(HookLifecycle::PostMemoryWrite),
        AgentEvent::ModelRequest { .. } | AgentEvent::ModelResponse { .. } => None,
        AgentEvent::Status(_)
        | AgentEvent::AssistantText(_)
        | AgentEvent::AssistantDelta(_)
        | AgentEvent::AssistantThinkingDelta(_)
        | AgentEvent::ToolProgress { .. }
        | AgentEvent::McpStatusUpdated(_)
        | AgentEvent::McpStatusLoadFailed { .. }
        | AgentEvent::TodoUpdated(_)
        | AgentEvent::PlanUpdated { .. }
        | AgentEvent::ApprovalRequested { .. }
        | AgentEvent::ApprovalAnswered { .. }
        | AgentEvent::AgentError { .. }
        | AgentEvent::Compaction { .. } => None,
    }
}

fn lifecycle_matches(lifecycle: &HookLifecycle, event: &AgentEvent) -> bool {
    lifecycle_for_event(event).as_ref() == Some(lifecycle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_event_bus::RuntimeEventBus;

    #[test]
    fn test_lifecycle_mapping_agent_start_is_none() {
        assert_eq!(lifecycle_for_event(&AgentEvent::AgentStart), None);
    }

    #[test]
    fn test_lifecycle_mapping_agent_stop_tool_use() {
        assert_eq!(
            lifecycle_for_event(&AgentEvent::AgentStop {
                reason: "done".into()
            }),
            Some(HookLifecycle::Stop)
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::ToolUse {
                call_id: "call-1".into(),
                name: "bash".into(),
                input: serde_json::json!({}),
            }),
            Some(HookLifecycle::PreToolUse)
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::ToolResult {
                call_id: "call-1".into(),
                name: "bash".into(),
                content: "ok".into(),
                is_error: false,
            }),
            Some(HookLifecycle::PostToolUse)
        );
    }

    #[test]
    fn test_non_mapped_events() {
        assert_eq!(
            lifecycle_for_event(&AgentEvent::Status("ready".into())),
            None
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::AssistantDelta("txt".into())),
            None
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::ModelRequest {
                model: "gpt-4".into(),
                input_tokens: 10,
            }),
            None
        );
    }

    #[tokio::test]
    async fn test_registers_hooks() {
        let bus = Arc::new(RuntimeEventBus::new(4));
        let runtime = HookRuntime::new(bus);

        assert_eq!(runtime.hook_count(), 0);

        runtime.register(
            "hook-1".into(),
            HookLifecycle::SessionStart,
            Box::new(|_| {}),
        );
        assert_eq!(runtime.hook_count(), 1);
    }

    #[tokio::test]
    async fn start_does_not_capture_runtime_event_bus_arc() {
        let bus = Arc::new(RuntimeEventBus::new(4));
        let runtime = HookRuntime::new(bus.clone());
        let before = Arc::strong_count(&bus);

        let handle = runtime.start().expect("start hook runtime");

        assert_eq!(Arc::strong_count(&bus), before);
        handle.abort();
    }

    #[test]
    fn dispatch_memory_query_invokes_memory_query_hooks() {
        let bus = Arc::new(RuntimeEventBus::new(4));
        let runtime = HookRuntime::new(bus);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_hook = seen.clone();
        runtime.register(
            "memory-query".into(),
            HookLifecycle::MemoryQuery,
            Box::new(move |event| {
                if let AgentEvent::MemoryAction { message } = event {
                    seen_hook.lock().expect("seen lock").push(message.clone());
                }
            }),
        );
        runtime.register(
            "pre-tool-use".into(),
            HookLifecycle::PreToolUse,
            Box::new(|_| panic!("pre-tool hook should not fire")),
        );

        runtime.dispatch_memory_query("repo notes");

        assert_eq!(
            seen.lock().expect("seen lock").as_slice(),
            ["MemoryQuery: repo notes"]
        );
    }
}
