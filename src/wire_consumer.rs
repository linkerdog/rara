//! Wire-mode adapter over `RuntimeSession` typed events.

use anyhow::Result;

use crate::agent::{AgentEvent, AgentOutputMode};
use crate::runtime_session::RuntimeSession;

/// Emits Wire JSON-RPC messages to stdout for each turn event.
pub struct WireConsumer {
    session: RuntimeSession,
    prompt: String,
}

impl WireConsumer {
    pub fn new(session: RuntimeSession, prompt: String) -> Self {
        Self { session, prompt }
    }

    /// Run the agent query and emit Wire messages to stdout.
    pub async fn run(self) -> Result<()> {
        let query_result = self
            .session
            .query_with_events(self.prompt, AgentOutputMode::Silent, |event| {
                Self::emit_wire(&event)
            })
            .await;
        let shutdown_result = self.session.shutdown().await;
        if let Err(error) = query_result {
            if let Err(shutdown_error) = shutdown_result {
                log::warn!("failed to shut down Wire runtime session: {shutdown_error}");
            }
            return Err(error.into());
        }
        shutdown_result?;
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
            AgentEvent::ToolUse {
                call_id,
                name,
                input,
            } => {
                let msg = serde_json::json!({
                    "type": "tool_use",
                    "call_id": call_id,
                    "name": name,
                    "input": input,
                });
                println!("{}", serde_json::to_string(&msg).unwrap_or_default());
            }
            AgentEvent::ToolResult {
                call_id,
                name,
                content,
                is_error,
            } => {
                let msg = serde_json::json!({
                    "type": "tool_result",
                    "call_id": call_id,
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
