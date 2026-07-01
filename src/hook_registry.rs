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
    pub lifecycle: HookLifecycle,
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
                description: _,
            } => {
                let entry = HookEntry {
                    lifecycle: *lifecycle,
                };
                self.hooks.write().await.insert(hook_id.clone(), entry);
                let _ = self
                    .event_bus
                    .publish_control(RuntimeEvent::Hook(HookEvent::Declared {
                        hook_id: hook_id.clone(),
                        lifecycle: *lifecycle,
                    }));
            }
            HookControlRequest::QueryHooks => {
                let hooks = self.hooks.read().await;
                for (hook_id, entry) in hooks.iter() {
                    let _ =
                        self.event_bus
                            .publish_control(RuntimeEvent::Hook(HookEvent::Declared {
                                hook_id: hook_id.clone(),
                                lifecycle: entry.lifecycle,
                            }));
                }
            }
        }
    }
}
