//! In-process hook runtime that subscribes to the RuntimeEventBus
//! and dispatches matching AgentEvent variants to registered hook callbacks.
//!
//! Hooks are declared through the control plane (HookControlRequest::Declare)
//! and can be registered at any time — before or after calling `start`.
//! The dispatch loop runs on a dedicated Tokio task and matches events
//! against the current set of registered hooks.

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
/// Hooks are stored behind an `Arc<RwLock<HashMap<...>>>` so that the
/// control-plane can register / unregister hooks while the dispatch
/// loop is already running.
pub struct HookRuntime {
    bus: Arc<RuntimeEventBus>,
    hooks: Arc<tokio::sync::RwLock<HashMap<String, HookEntry>>>,
}

impl HookRuntime {
    pub fn new(bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            bus,
            hooks: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Register an in-process hook.  Safe to call while `start` is running.
    pub async fn register(
        &self,
        hook_id: String,
        lifecycle: HookLifecycle,
        description: String,
        callback: HookCallback,
    ) {
        self.hooks.write().await.insert(
            hook_id,
            HookEntry {
                lifecycle,
                description,
                callback,
            },
        );
    }

    /// Unregister a previously declared hook by id.
    pub async fn unregister(&self, hook_id: &str) -> bool {
        self.hooks.write().await.remove(hook_id).is_some()
    }

    /// Return the number of registered hooks.
    pub async fn hook_count(&self) -> usize {
        self.hooks.read().await.len()
    }

    /// Start the hook dispatch loop on a dedicated Tokio task.
    ///
    /// The returned handle can be aborted to stop hook processing.
    /// Callers may safely call `register` / `unregister` after this returns.
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        let hooks = Arc::clone(&self.hooks);
        let bus = self.bus.clone();

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

    #[tokio::test]
    async fn test_register_and_unregister() {
        let bus = Arc::new(RuntimeEventBus::new(4));
        let runtime = HookRuntime::new(bus);

        assert_eq!(runtime.hook_count().await, 0);

        runtime
            .register(
                "hook-1".into(),
                HookLifecycle::SessionStart,
                "test hook".into(),
                Box::new(|_| {}),
            )
            .await;
        assert_eq!(runtime.hook_count().await, 1);

        assert!(runtime.unregister("hook-1").await);
        assert_eq!(runtime.hook_count().await, 0);

        assert!(!runtime.unregister("hook-1").await);
    }
}
