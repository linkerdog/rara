//! MCP connection manager.
//!
//! Manages lifecycle of external MCP server processes: starting, health-checking,
//! reconnecting, and emitting status events through the runtime event bus.
//!
//! ## Design
//!
//! - **State machine per server**: `Configured → Connecting → Connected → Disconnected`
//! - **Refresh**: compares `McpRegistry` against running servers; starts added
//!   configs, stops removed configs, reconciles changed configs.
//! - **Reconnect**: exponential backoff (1s → 2s → 4s … capped at 64s)
//!   when a server process exits or becomes unhealthy.
//! - **Event emission**: publishes `McpEvent` variants via `RuntimeEventBus`.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::config::{McpRegistry, SourcedMcpServerConfig};
use crate::mcp_status::McpConnectionState;
use crate::mcp_tool_cache::McpToolCache;
use crate::runtime_control::{McpControlRequest, McpEvent, RuntimeEvent};
use crate::runtime_event_bus::RuntimeEventBus;

/// Per-server tracking kept by `McpConnectionManager`.
struct McpServerEntry {
    config: SourcedMcpServerConfig,
    state: McpConnectionState,
    /// Consecutive connection failures, for backoff calculation.
    consecutive_failures: u32,
}

impl McpServerEntry {
    fn backoff_duration(&self) -> std::time::Duration {
        let base_secs = match self.consecutive_failures.min(6) {
            0 => 1,
            n => 1u64 << n,
        };
        std::time::Duration::from_secs(base_secs)
    }

    fn max_backoff() -> std::time::Duration {
        std::time::Duration::from_secs(64)
    }
}

/// Manages MCP server connections.
///
/// Created once at bootstrap and held for the runtime lifetime.
/// Callers drive it via `handle_control()` and `refresh()`.
pub struct McpConnectionManager {
    registry: Arc<McpRegistry>,
    event_bus: Arc<RuntimeEventBus>,
    entries: RwLock<Vec<(String, McpServerEntry)>>,
    tool_cache: McpToolCache,
}

impl McpConnectionManager {
    /// Create a new manager from the current MCP registry and event bus.
    pub fn new(
        registry: Arc<McpRegistry>,
        event_bus: Arc<RuntimeEventBus>,
        tool_cache: McpToolCache,
    ) -> Self {
        Self {
            registry,
            event_bus,
            entries: RwLock::new(Vec::new()),
            tool_cache,
        }
    }

    /// Compare the registry against tracked servers and reconcile.
    ///
    /// - Configs added since last refresh → mark as `Connecting`.
    /// - Configs removed → mark as `Disconnected`.
    /// - Configs whose transport changed → re-mark as `Connecting`.
    pub async fn refresh(&self) {
        let registry_servers = &self.registry.servers;
        let mut entries = self.entries.write().await;

        // Remove servers no longer in config.
        entries.retain(|(name, _entry)| {
            let keep = registry_servers.contains_key(name);
            if !keep {
                self.emit_state_change(name, McpConnectionState::Disconnected);
            }
            keep
        });

        for (name, config) in registry_servers {
            let state = match entries.iter().find(|(n, _)| n == name) {
                Some((_, existing)) if existing.config.config == config.config => {
                    // Unchanged config — keep current state.
                    continue;
                }
                _ => {
                    // New or changed config.
                    McpConnectionState::Connecting
                }
            };

            self.emit_state_change(name, state);
            entries.push((
                name.clone(),
                McpServerEntry {
                    config: config.clone(),
                    state,
                    consecutive_failures: 0,
                },
            ));
        }
    }

    /// Request a reconnect for a specific server (exponential backoff).
    pub async fn reconnect(&self, server_name: &str) {
        let mut entries = self.entries.write().await;
        let Some(idx) = entries.iter().position(|(n, _)| n == server_name) else {
            return;
        };

        let entry = &mut entries[idx].1;
        entry.consecutive_failures += 1;
        let backoff = entry.backoff_duration().min(McpServerEntry::max_backoff());

        entry.state = McpConnectionState::Reconnecting;

        let _ = self
            .event_bus
            .publish_control(RuntimeEvent::Mcp(McpEvent::ServerReconnecting {
                server_name: server_name.into(),
                attempt: entry.consecutive_failures + 1,
                backoff_ms: backoff.as_millis() as u64,
            }));

        self.emit_state_change(server_name, McpConnectionState::Reconnecting);
    }

    /// Handle an MCP control request.
    pub async fn handle_control(&self, request: &McpControlRequest) {
        match request {
            McpControlRequest::QueryStatus => {
                let snapshot = crate::mcp_status::McpStatusSnapshot::from_registry(&self.registry);
                let _ = self
                    .event_bus
                    .publish_control(RuntimeEvent::Mcp(McpEvent::StatusUpdated { snapshot }));
                for (name, entry) in self.entries.read().await.iter() {
                    self.emit_state_change(name, entry.state);
                }
            }
            McpControlRequest::Refresh { server_name } => {
                if let Some(name) = server_name {
                    self.reconnect(name).await;
                } else {
                    self.refresh().await;
                }
                let _ = self
                    .event_bus
                    .publish_control(RuntimeEvent::Mcp(McpEvent::ConfigurationRefreshed));
            }
            McpControlRequest::Reconnect { server_name } => {
                self.reconnect(server_name).await;
            }
        }
    }

    fn emit_state_change(&self, server_name: &str, state: McpConnectionState) {
        let _ = self
            .event_bus
            .publish_control(RuntimeEvent::Mcp(McpEvent::ServerStateChanged {
                server_name: server_name.into(),
                state,
            }));
    }
}
