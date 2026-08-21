//! Print-mode adapter over `RuntimeSession` typed events. No TUI, no JSON.

use anyhow::Result;

use crate::agent::{AgentEvent, AgentOutputMode};
use crate::runtime_session::RuntimeSession;

pub struct PrintConsumer {
    session: RuntimeSession,
    prompt: String,
}

impl PrintConsumer {
    pub fn new(session: RuntimeSession, prompt: String) -> Self {
        Self { session, prompt }
    }

    /// Run the agent query and print streaming output until completion.
    pub async fn run(self) -> Result<()> {
        let query_result = self
            .session
            .query_with_events(self.prompt, AgentOutputMode::Silent, |event| {
                Self::render_event(&event)
            })
            .await;
        let shutdown_result = self.session.shutdown().await;
        if let Err(error) = query_result {
            if let Err(shutdown_error) = shutdown_result {
                log::warn!("failed to shut down print runtime session: {shutdown_error}");
            }
            return Err(error.into());
        }
        shutdown_result?;
        println!(); // trailing newline
        Ok(())
    }

    fn render_event(event: &AgentEvent) {
        match event {
            AgentEvent::AssistantDelta(text) | AgentEvent::AssistantText(text) => {
                print!("{text}");
            }
            AgentEvent::ToolUse { name, .. } => {
                eprintln!("\n[Tool: {name}]");
            }
            AgentEvent::ToolResult { name, is_error, .. } if *is_error => {
                eprintln!("\n[Tool error: {name}]");
            }
            AgentEvent::Status(msg) => {
                eprintln!("\n[{msg}]");
            }
            _ => {}
        }
    }
}
