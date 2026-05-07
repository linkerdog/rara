//! Print-mode consumer: subscribes to RuntimeEventBus and renders
//! AgentEvent as plain text to stdout/stderr. No TUI, no JSON.
//!
//! The consumer owns the agent lifecycle: it spawns the query task
//! and consumes events until the task completes.

use std::sync::Arc;

use anyhow::Result;

use crate::agent::Agent;
use crate::agent::AgentEvent;
use crate::runtime_event_bus::RuntimeEventBus;

pub struct PrintConsumer {
    agent: Agent,
    event_bus: Arc<RuntimeEventBus>,
    prompt: String,
}

impl PrintConsumer {
    pub fn new(agent: Agent, event_bus: Arc<RuntimeEventBus>, prompt: String) -> Self {
        Self {
            agent,
            event_bus,
            prompt,
        }
    }

    /// Run the agent query and print streaming output until completion.
    pub async fn run(mut self) -> Result<()> {
        let mut rx = self.event_bus.subscribe();
        let handle = tokio::spawn(async move { self.agent.query(self.prompt).await });

        while let Ok(event) = rx.recv().await {
            Self::render_event(&event);
        }

        let _ = handle.await?;
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
            AgentEvent::ToolResult { name, is_error, .. } => {
                if *is_error {
                    eprintln!("\n[Tool error: {name}]");
                }
            }
            AgentEvent::Status(msg) => {
                eprintln!("\n[{msg}]");
            }
            _ => {}
        }
    }
}
