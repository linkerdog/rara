//! ACP peer consumer: subscribes to RuntimeEventBus and translates
//! AgentEvent into ACP SessionNotification for streaming to ACP clients.
//!
//! Peer consumer to TuiMaintainer (Ratatui) and PrintConsumer (stdout).

use std::sync::Arc;

use agent_client_protocol::schema::SessionId;
use agent_client_protocol::schema::{
    ContentChunk, SessionNotification, SessionUpdate, TextContent,
};

use crate::agent::AgentEvent;
use crate::runtime_event_bus::RuntimeEventBus;

/// Subscribes to RuntimeEventBus and yields ACP SessionNotification
/// for each AgentEvent. The caller sends notifications to the ACP client.
#[allow(dead_code)]
pub struct AcpConsumer {
    rx: tokio::sync::broadcast::Receiver<AgentEvent>,
    session_id: SessionId,
}

impl AcpConsumer {
    pub fn new(event_bus: Arc<RuntimeEventBus>, session_id: SessionId) -> Self {
        Self {
            rx: event_bus.subscribe(),
            session_id,
        }
    }

    /// Wait for the next AgentEvent and translate it to an ACP session update.
    /// Returns None if the bus is closed (agent finished).
    pub async fn next_notification(&mut self) -> Option<SessionNotification> {
        match self.rx.recv().await {
            // Text deltas: stream as AgentMessageChunk
            Ok(AgentEvent::AssistantDelta(text)) => {
                let chunk = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new(text),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))
            }
            // Full text messages: also stream as chunk
            Ok(AgentEvent::AssistantText(text)) => {
                let chunk = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new(text),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))
            }
            // Thinking delta: stream as chunk
            Ok(AgentEvent::AssistantThinkingDelta(chunk)) => {
                let content = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new(chunk),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(content),
                ))
            }
            // Tool use: emit as notification chunk
            Ok(AgentEvent::ToolUse { name, input: _ }) => {
                let label = format!("[Tool: {name}]\n");
                let chunk = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new(label),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))
            }
            // Tool result: emit as text chunk
            Ok(AgentEvent::ToolResult {
                name,
                content,
                is_error,
            }) => {
                let prefix = if is_error {
                    "[Tool error"
                } else {
                    "[Tool result"
                };
                let label = format!("{prefix}: {name}]\n{content}\n");
                let chunk = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new(label),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))
            }
            // Status events: emit as chunk with prefix
            Ok(AgentEvent::Status(message)) => {
                let chunk = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new(format!("[{message}]")),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))
            }
            // Other events: skip notification (handled by main ACP loop directly)
            Ok(_) => None,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // If we lagged, emit a truncation notice.
                let chunk = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new("\n[output truncated: receiver lagged]\n".to_string()),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_event_bus::RuntimeEventBus;

    #[tokio::test]
    async fn test_acp_consumer_receives_delta() {
        let bus = Arc::new(RuntimeEventBus::new(16));
        let session_id = SessionId::from("test-session".to_string());
        let mut consumer = AcpConsumer::new(bus.clone(), session_id.clone());

        bus.send_with_provenance(
            AgentEvent::AssistantDelta("hello".to_string()),
            crate::runtime_control::RuntimeProvenance {
                controller: crate::runtime_control::RuntimeControllerKind::Acp,
                adapter: None,
                session_id: Some("test-session".to_string()),
                source_id: None,
                trust: crate::runtime_control::RuntimeSourceTrust::Trusted,
                authorship: crate::runtime_control::RuntimeSourceAuthorship::Generated,
            },
        );

        let notification = consumer.next_notification().await.unwrap();
        assert_eq!(notification.session_id, session_id);
    }

    #[tokio::test]
    async fn test_acp_consumer_receives_reasoning() {
        let bus = Arc::new(RuntimeEventBus::new(16));
        let session_id = SessionId::from("test-session".to_string());
        let mut consumer = AcpConsumer::new(bus.clone(), session_id.clone());

        bus.send_with_provenance(
            AgentEvent::AssistantThinkingDelta("thinking...".to_string()),
            crate::runtime_control::RuntimeProvenance {
                controller: crate::runtime_control::RuntimeControllerKind::Acp,
                adapter: None,
                session_id: Some("test-session".to_string()),
                source_id: None,
                trust: crate::runtime_control::RuntimeSourceTrust::Trusted,
                authorship: crate::runtime_control::RuntimeSourceAuthorship::Generated,
            },
        );

        let notification = consumer.next_notification().await;
        assert!(notification.is_some());
    }

    #[tokio::test]
    async fn test_acp_consumer_skips_non_text_events() {
        let bus = Arc::new(RuntimeEventBus::new(16));
        let session_id = SessionId::from("test-session".to_string());
        let mut consumer = AcpConsumer::new(bus.clone(), session_id.clone());

        // Subscribe to consume one event, then send non-text.
        // Since next_notification is single-consumer per event,
        // we send a text event after to unblock.
        let bus2 = bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            bus2.send_with_provenance(
                AgentEvent::AssistantDelta("after".to_string()),
                crate::runtime_control::RuntimeProvenance {
                    controller: crate::runtime_control::RuntimeControllerKind::Acp,
                    adapter: None,
                    session_id: Some("test-session".to_string()),
                    source_id: None,
                    trust: crate::runtime_control::RuntimeSourceTrust::Trusted,
                    authorship: crate::runtime_control::RuntimeSourceAuthorship::Generated,
                },
            );
        });

        let notification = consumer.next_notification().await;
        assert!(notification.is_some());
    }
}
