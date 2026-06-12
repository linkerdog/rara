//! Hook declaration registry (control-plane facing).
//!
//! Stores hook declarations registered by protocol adapters through the
//! control plane. This is declaration-only scaffolding — hooks are recorded
//! and query-able but not yet executed. Execution will be enabled once the
//! permission/authorization model is defined.
//!
//! This module complements `hooks.rs` (file-system discovery) by providing
//! the protocol-driven registration path via `HookControlRequest`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::runtime_control::{HookControlRequest, HookEvent, HookLifecycle, RuntimeEvent};
use crate::runtime_event_bus::RuntimeEventBus;

/// A stored hook declaration.
#[derive(Clone, Debug)]
pub struct HookEntry {
    pub id: String,
    pub lifecycle: HookLifecycle,
    pub description: String,
}

/// Registry of control-plane-declared hooks.
///
/// Declarations are received via the control plane and persisted only in
/// memory (cleared on restart). Execution is not wired yet.
pub struct HookRegistry {
    hooks: RwLock<HashMap<String, HookEntry>>,
    event_bus: Arc<RuntimeEventBus>,
}

impl HookRegistry {
    pub fn new(event_bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            hooks: RwLock::new(HashMap::new()),
            event_bus,
        }
    }

    /// Handle a control-plane hook request.
    pub async fn handle_control(&self, request: &HookControlRequest) {
        match request {
            HookControlRequest::Declare {
                hook_id,
                lifecycle,
                description,
            } => {
                let entry = HookEntry {
                    id: hook_id.clone(),
                    lifecycle: lifecycle.clone(),
                    description: description.clone(),
                };
                self.hooks.write().await.insert(hook_id.clone(), entry);
                let _ = self
                    .event_bus
                    .publish_control(RuntimeEvent::Hook(HookEvent::Declared {
                        hook_id: hook_id.clone(),
                        lifecycle: lifecycle.clone(),
                    }));
            }
            HookControlRequest::QueryHooks => {
                let hooks = self.hooks.read().await;
                for (hook_id, entry) in hooks.iter() {
                    let _ =
                        self.event_bus
                            .publish_control(RuntimeEvent::Hook(HookEvent::Declared {
                                hook_id: hook_id.clone(),
                                lifecycle: entry.lifecycle.clone(),
                            }));
                }
            }
        }
    }

    /// Return a snapshot of all registered hooks.
    pub async fn all_hooks(&self) -> Vec<HookEntry> {
        self.hooks.read().await.values().cloned().collect()
    }

    /// Return hooks registered for a given lifecycle phase (non-async).
    pub fn hooks_for_phase(&self, phase: HookLifecycle) -> Vec<HookEntry> {
        self.hooks
            .try_read()
            .map(|guard| {
                guard
                    .values()
                    .filter(|h| h.lifecycle == phase)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}
