//! In-process hook runtime that subscribes to the RuntimeEventBus
//! and dispatches matching AgentEvent variants to registered hook callbacks.
//!
//! Hooks are declared through the control plane (HookControlRequest::Declare)
//! and executed synchronously in the hook task when a matching lifecycle
//! event occurs.

use std::collections::HashMap;
use std::sync::Arc;

use crate::agent::AgentEvent;
use crate::runtime_control::HookLifecycle;
use crate::runtime_event_bus::RuntimeEventBus;

/// A registered in-process hook combining a lifecycle trigger and a callback.
type HookCallback = Box<dyn Fn(&AgentEvent) + Send + Sync>;

struct HookEntry {
    lifecycle: HookLifecycle,
    description: String,
    callback: HookCallback,
}

/// In-process hook dispatch runtime.
///
/// Subscribes to the agent-level event bus and runs matching hook callbacks
/// on a dedicated Tokio task. This initial version only supports in-process
/// callbacks; external hook runners (e.g. wasm, standalone processes) will
/// be added later.
pub struct HookRuntime {
    bus: Arc<RuntimeEventBus>,
    hooks: HashMap<String, HookEntry>,
}

impl HookRuntime {
    pub fn new(bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            bus,
            hooks: HashMap::new(),
        }
    }

    /// Register an in-process hook.
    ///
    /// `hook_id` must be unique across all declared hooks. When a matching
    /// `lifecycle` event fires the `callback` is invoked with the event.
    pub fn register(
        &mut self,
        hook_id: String,
        lifecycle: HookLifecycle,
        description: String,
        callback: HookCallback,
    ) {
        self.hooks.insert(
            hook_id,
            HookEntry {
                lifecycle,
                description,
                callback,
            },
        );
    }

    /// Unregister a previously declared hook by id.
    pub fn unregister(&mut self, hook_id: &str) -> bool {
        self.hooks.remove(hook_id).is_some()
    }

    /// Return the number of registered hooks.
    pub fn hook_count(&self) -> usize {
        self.hooks.len()
    }

    /// Start the hook dispatch loop on a dedicated Tokio task.
    ///
    /// The returned handle can be aborted to stop hook processing.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        let hooks = Arc::new(tokio::sync::RwLock::new(self.hooks));
        let bus = self.bus;

        tokio::task::spawn(async move {
            let mut rx = bus.subscribe();
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let guard = hooks.read().await;
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
        })
    }
}

/// Map an AgentEvent variant to the HookLifecycle it corresponds to.
fn lifecycle_for_event(event: &AgentEvent) -> Option<HookLifecycle> {
    match event {
        AgentEvent::AgentStart => Some(HookLifecycle::SessionStart),
        AgentEvent::AgentStop { .. } => Some(HookLifecycle::Stop),
        AgentEvent::ToolUse { .. } => Some(HookLifecycle::PreToolUse),
        AgentEvent::ToolResult { .. } => Some(HookLifecycle::PostToolUse),
        AgentEvent::ModelRequest { .. } | AgentEvent::ModelResponse { .. } => None,
        AgentEvent::Status(_)
        | AgentEvent::AssistantText(_)
        | AgentEvent::AssistantDelta(_)
        | AgentEvent::AssistantThinkingDelta(_)
        | AgentEvent::ToolProgress { .. }
        | AgentEvent::McpStatusUpdated(_)
        | AgentEvent::McpStatusLoadFailed { .. }
        | AgentEvent::TodoUpdated(_)
        | AgentEvent::AgentError { .. } => None,
    }
}

fn lifecycle_matches(lifecycle: &HookLifecycle, event: &AgentEvent) -> bool {
    lifecycle_for_event(event) == Some(lifecycle.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_event_bus::RuntimeEventBus;

    #[test]
    fn test_lifecycle_mapping() {
        assert_eq!(
            lifecycle_for_event(&AgentEvent::AgentStart),
            Some(HookLifecycle::SessionStart)
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::AgentStop {
                reason: "done".into()
            }),
            Some(HookLifecycle::Stop)
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::ToolUse {
                name: "bash".into(),
                input: serde_json::json!({}),
            }),
            Some(HookLifecycle::PreToolUse)
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::ToolResult {
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

    #[test]
    fn test_register_and_unregister() {
        let bus = Arc::new(RuntimeEventBus::new(4));
        let mut runtime = HookRuntime::new(bus);

        assert_eq!(runtime.hook_count(), 0);

        runtime.register(
            "hook-1".into(),
            HookLifecycle::SessionStart,
            "test hook".into(),
            Box::new(|_| {}),
        );
        assert_eq!(runtime.hook_count(), 1);

        assert!(runtime.unregister("hook-1"));
        assert_eq!(runtime.hook_count(), 0);

        assert!(!runtime.unregister("hook-1"));
    }
}
