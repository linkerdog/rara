//! ACP (Agent Client Protocol) integration for RARA.
//!
//! Implements an ACP-compliant agent that responds to prompts using the
//! RARA LLM backend. The agent accepts stdio transports per the ACP spec.

use std::sync::Arc;

use agent_client_protocol::{
    schema::{
        AgentCapabilities, AuthenticateRequest, AuthenticateResponse,
        CancelNotification, ContentChunk, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
        SessionId, SessionNotification, SessionUpdate, StopReason, TextContent,
    },
    Agent, Client, ConnectionTo, Dispatch, Responder, Result as AcpResult,
};
use tokio::io::{stdin, stdout};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::llm::LlmBackend;
use crate::llm::LlmStreamEvent;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tool::ToolManager;

/// ACP agent state shared across request handlers.
pub struct RaraAcpAgent {
    pub llm_backend: Arc<dyn LlmBackend>,
    pub tool_manager: Arc<ToolManager>,
    pub event_bus: Arc<RuntimeEventBus>,
}

impl RaraAcpAgent {
    pub fn new(
        llm_backend: Arc<dyn LlmBackend>,
        tool_manager: Arc<ToolManager>,
        event_bus: Arc<RuntimeEventBus>,
    ) -> Self {
        Self {
            llm_backend,
            tool_manager,
            event_bus,
        }
    }

    /// Run the ACP agent on stdio using the ACP v0.11 builder API.
    pub async fn run_acp_stdio(self) -> AcpResult<()> {
        let llm = self.llm_backend.clone();
        let _tools = self.tool_manager.clone();
        let _bus = self.event_bus.clone();

        let transport = agent_client_protocol::ByteStreams::new(
            stdout().compat_write(),
            stdin().compat(),
        );

        Agent
            .builder()
            .name("rara")
            .on_receive_request(
                async move |initialize: InitializeRequest, responder, _cx| {
                    responder.respond(
                        InitializeResponse::new(initialize.protocol_version)
                            .agent_capabilities(AgentCapabilities::new()),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |_req: AuthenticateRequest, responder, _cx| {
                    responder.respond(AuthenticateResponse::new())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |_req: NewSessionRequest, responder, _cx| {
                    let session_id = SessionId::from(uuid::Uuid::new_v4().to_string());
                    responder.respond(NewSessionResponse::new(session_id))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let llm = llm.clone();
                    async move |req: PromptRequest, responder, cx: ConnectionTo<Client>| {
                        handle_prompt(req, responder, cx, &*llm).await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |_notif: CancelNotification, _cx: ConnectionTo<Client>| {
                    eprintln!("[acp] cancel notification received");
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_dispatch(
                async move |message: Dispatch, cx: ConnectionTo<Client>| {
                    message.respond_with_error(
                        agent_client_protocol::util::internal_error("unhandled ACP message"),
                        cx,
                    )
                },
                agent_client_protocol::on_receive_dispatch!(),
            )
            .connect_to(transport)
            .await
    }
}

/// Extract plain text from ACP content blocks for the LLM prompt.
fn extract_prompt_text(blocks: &[agent_client_protocol::schema::ContentBlock]) -> String {
    use agent_client_protocol::schema::ContentBlock;

    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Handle a PromptRequest: stream LLM output as ACP SessionNotifications,
/// then respond with a PromptResponse.
async fn handle_prompt(
    req: PromptRequest,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
    llm: &dyn LlmBackend,
) -> AcpResult<()> {
    let session_id = req.session_id.clone();
    let prompt_text = extract_prompt_text(&req.prompt);

    if prompt_text.trim().is_empty() {
        responder.respond(PromptResponse::new(StopReason::EndTurn))?;
        return Ok(());
    }

    let messages = vec![crate::agent::Message {
        role: "user".to_string(),
        content: serde_json::Value::String(prompt_text),
    }];

    let cx_for_callback = cx.clone();
    let mut on_event = {
        let session_id = session_id.clone();
        move |event: LlmStreamEvent| {
            if let LlmStreamEvent::TextDelta(text) = event {
                let chunk = ContentChunk::new(
                    agent_client_protocol::schema::ContentBlock::Text(TextContent::new(text)),
                );
                let _ = cx_for_callback.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ));
            }
        }
    };

    match llm.ask_streaming(&messages, &[], &mut on_event).await {
        Ok(_response) => {
            responder.respond(PromptResponse::new(StopReason::EndTurn))?;
        }
        Err(e) => {
            eprintln!("[acp] LLM streaming error: {e:?}");
            let chunk = ContentChunk::new(
                agent_client_protocol::schema::ContentBlock::Text(TextContent::new(format!("LLM error: {e}"))),
            );
            let _ = cx.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::AgentMessageChunk(chunk),
            ));
            responder.respond(PromptResponse::new(StopReason::EndTurn))?;
        }
    }

    Ok(())
}
