//! Wire-mode consumer: subscribes to RuntimeEventBus and renders
//! AgentEvent as Wire JSON-RPC over stdout. Peer consumer to
//! PrintConsumer (text), AcpConsumer (ACP), and TuiMaintainer (Ratatui).
//!
//! All four consumers subscribe to the same event bus.

use std::sync::Arc;

use anyhow::Result;

use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::runtime_event_bus::RuntimeEventBus;

/// Spawns the agent query and emits Wire JSON-RPC messages to stdout
/// for each AgentEvent until completion.
pub struct WireConsumer {
    agent: Agent,
    event_bus: Arc<RuntimeEventBus>,
    prompt: String,
}

impl WireConsumer {
    pub fn new(agent: Agent, event_bus: Arc<RuntimeEventBus>, prompt: String) -> Self {
        Self {
            agent,
            event_bus,
            prompt,
        }
    }

    /// Run the agent query and emit Wire messages to stdout.
    pub async fn run(mut self) -> Result<()> {
        let mut rx = self.event_bus.subscribe();
        // Spawn agent in background; it publishes AgentEvent to the bus.
        let handle = tokio::spawn(async move { self.agent.query(self.prompt).await });

        while let Ok(event) = rx.recv().await {
            Self::emit_wire(&event);
        }

        let _ = handle.await?;
        Ok(())
    }

    fn emit_wire(event: &AgentEvent) {
        match event {
            AgentEvent::AssistantDelta(text) => {
                // Wire: incremental text chunk
                let msg = serde_json::json!({
                    "type": "assistant_delta",
                    "text": text,
                });
                println!("{}", serde_json::to_string(&msg).unwrap_or_default());
            }
            AgentEvent::AssistantText(text) => {
                let msg = serde_json::json!({
                    "type": "assistant_message",
                    "text": text,
                });
                println!("{}", serde_json::to_string(&msg).unwrap_or_default());
            }
            AgentEvent::ToolUse { name, input } => {
                let msg = serde_json::json!({
                    "type": "tool_use",
                    "name": name,
                    "input": input,
                });
                println!("{}", serde_json::to_string(&msg).unwrap_or_default());
            }
            AgentEvent::ToolResult {
                name,
                content,
                is_error,
            } => {
                let msg = serde_json::json!({
                    "type": "tool_result",
                    "name": name,
                    "content": content,
                    "is_error": is_error,
                });
                println!("{}", serde_json::to_string(&msg).unwrap_or_default());
            }
            AgentEvent::Status(msg) => {
                let wire = serde_json::json!({
                    "type": "status",
                    "message": msg,
                });
                println!("{}", serde_json::to_string(&wire).unwrap_or_default());
            }
            _ => {}
        }
    }
}
