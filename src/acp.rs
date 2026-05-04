use std::sync::Arc;

use agent_client_protocol::Error;
use agent_client_protocol::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, Implementation,
    InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, ProtocolVersion, SessionId, StopReason,
};

use crate::llm::LlmBackend;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tool::ToolManager;

pub struct RaraAcpAgent {
    pub tool_manager: ToolManager,
    pub backend_builder: Box<dyn Fn() -> Box<dyn LlmBackend> + Send + Sync>,
    /// Runtime event bus for subscribing to AgentEvent streams during
    /// prompt turns.  Shared with the TUI runtime when running in the
    /// same process.
    pub event_bus: Arc<RuntimeEventBus>,
}

impl RaraAcpAgent {
    pub async fn initialize(&self, _: InitializeRequest) -> Result<InitializeResponse, Error> {
        Ok(InitializeResponse::new(ProtocolVersion::V1)
            .agent_info(Implementation::new("rara", "0.1.0")))
    }

    pub async fn authenticate(
        &self,
        _: AuthenticateRequest,
    ) -> Result<AuthenticateResponse, Error> {
        Err(Error::method_not_found())
    }

    pub async fn new_session(&self, _: NewSessionRequest) -> Result<NewSessionResponse, Error> {
        Ok(NewSessionResponse::new(SessionId::new(
            "default".to_string(),
        )))
    }

    pub async fn prompt(&self, _: PromptRequest) -> Result<PromptResponse, Error> {
        // TODO: spawn agent turn via self.event_bus, translate AgentEvent
        // stream into ACP PromptResponse content blocks, and return the final
        // response with StopReason::EndTurn.
        let _sub = self.event_bus.subscribe();
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    pub async fn cancel(&self, _: CancelNotification) -> Result<(), Error> {
        Ok(())
    }
}

pub async fn run_acp_stdio(_agent: RaraAcpAgent) -> anyhow::Result<()> {
    // Runner implementation depends on the exact crate structure
    Ok(())
}
