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
            Ok(AgentEvent::AssistantDelta(text)) => {
                let chunk = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new(text),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))
            }
            Ok(AgentEvent::AssistantText(text)) => {
                let chunk = ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                    TextContent::new(text),
                ));
                Some(SessionNotification::new(
                    self.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))
            }
            Ok(_) => None, // non-text events: skip notification
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
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

        // Publish an event to the bus — simulates agent output.
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

        // Consumer should receive and translate it.
        let notification = consumer.next_notification().await.unwrap();
        assert_eq!(notification.session_id, session_id);
    }
}
