//! ACP (Agent Client Protocol) integration for RARA.
//!
//! Implements an ACP-compliant agent with a full tool-calling loop.
//! The agent accepts stdio transports per the ACP spec.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_client_protocol::{
    Agent as AcpRoleAgent, Client, ConnectionTo, Dispatch, Responder, Result as AcpResult,
    schema::v1::{
        AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification,
        ContentBlock as AcpContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionId,
        SessionNotification, SessionUpdate, StopReason, TextContent,
    },
};
use rara_tools::tool::{ToolManager, ToolProgressEvent};
use serde_json::Value;
use tokio::io::{stdin, stdout};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::agent::{Agent, Message};
use crate::hook_registry::HookRegistry;
use crate::llm::{ContentBlock, LlmBackend, LlmStreamEvent};
use crate::mcp_connection_manager::McpConnectionManager;
use crate::protocol_sources::{MemoryControlHandler, PromptSourceRegistry, SkillSourceRegistry};
use crate::runtime_control::{
    RuntimeControlEnvelope, RuntimeControlRequest, RuntimeControllerKind,
};
use crate::runtime_event_bus::RuntimeEventBus;

/// ACP agent state shared across request handlers.
pub struct RaraAcpAgent {
    pub llm_backend: Arc<dyn LlmBackend>,
    pub tool_manager: Arc<ToolManager>,
    pub event_bus: Arc<RuntimeEventBus>,
    pub mcp_manager: Arc<McpConnectionManager>,
    pub prompt_registry: Arc<PromptSourceRegistry>,
    pub skill_registry: Arc<SkillSourceRegistry>,
    pub hook_registry: Arc<HookRegistry>,
    pub memory_handler: Arc<MemoryControlHandler>,
    /// active agent (one per ACP session for now)
    active_agent: tokio::sync::Mutex<Option<Agent>>,
}

impl RaraAcpAgent {
    #[allow(clippy::too_many_arguments)]
    // ACP session wiring owns the runtime dependencies explicitly; grouping
    // them would duplicate RuntimeContext without reducing call-site risk.
    pub fn new(
        llm_backend: Arc<dyn LlmBackend>,
        tool_manager: Arc<ToolManager>,
        event_bus: Arc<RuntimeEventBus>,
        mcp_manager: Arc<McpConnectionManager>,
        prompt_registry: Arc<PromptSourceRegistry>,
        skill_registry: Arc<SkillSourceRegistry>,
        hook_registry: Arc<HookRegistry>,
        memory_handler: Arc<MemoryControlHandler>,
    ) -> Self {
        Self {
            llm_backend,
            tool_manager,
            event_bus,
            mcp_manager,
            prompt_registry,
            skill_registry,
            hook_registry,
            memory_handler,
            active_agent: tokio::sync::Mutex::new(None),
        }
    }

    /// Run the ACP agent on stdio using the ACP builder API.
    pub async fn run_acp_stdio(self) -> AcpResult<()> {
        let llm = self.llm_backend.clone();
        let tools = self.tool_manager.clone();
        let this = Arc::new(self);

        let transport =
            agent_client_protocol::ByteStreams::new(stdout().compat_write(), stdin().compat());

        AcpRoleAgent::builder(AcpRoleAgent)
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
                    let this = this.clone();
                    let llm = llm.clone();
                    let tools = tools.clone();
                    move |req: PromptRequest, responder, cx: ConnectionTo<Client>| {
                        let this = this.clone();
                        let llm = llm.clone();
                        let tools = tools.clone();
                        async move { this.handle_prompt(req, responder, cx, llm, tools).await }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                {
                    let this = this.clone();
                    async move |_notif: CancelNotification, _cx: ConnectionTo<Client>| {
                        eprintln!("[acp] cancel notification received");

                        let provenance = crate::runtime_control::RuntimeProvenance::protocol(
                            crate::runtime_control::RuntimeControllerKind::Acp,
                            "acp",
                            None, // We might not have session_id here if it's broad?
                            // Actually ACP CancelNotification has session_id.
                            None,
                        );
                        let request = crate::runtime_control::RuntimeControlRequest::Session(
                            crate::runtime_control::SessionControlRequest::CancelCurrentTurn,
                        );
                        let envelope = crate::runtime_control::RuntimeControlEnvelope {
                            request_id: uuid::Uuid::new_v4().to_string(),
                            provenance,
                            request,
                        };

                        let mut active_agent = this.active_agent.lock().await;
                        let agent = active_agent.as_mut();

                        let _ = crate::control_plane::dispatch(
                            envelope,
                            &this.mcp_manager,
                            &this.prompt_registry,
                            &this.skill_registry,
                            &this.memory_handler,
                            &this.hook_registry,
                            agent,
                            |_| {},
                        )
                        .await;

                        Ok(())
                    }
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

    /// Handle a prompt request with a full tool-calling agent loop.
    async fn handle_prompt(
        &self,
        req: PromptRequest,
        responder: Responder<PromptResponse>,
        cx: ConnectionTo<Client>,
        _llm: Arc<dyn LlmBackend>,
        _tool_manager: Arc<ToolManager>,
    ) -> AcpResult<()> {
        let session_id = req.session_id.to_string();
        let prompt_text = extract_prompt_text(&req.prompt);

        if prompt_text.trim().is_empty() {
            responder.respond(PromptResponse::new(StopReason::EndTurn))?;
            return Ok(());
        }

        let cancel_token = Arc::new(AtomicBool::new(false));

        // Create or get the agent for this session.
        let mut active_agent = self.active_agent.lock().await;
        if active_agent.is_none() {
            let bootstrap = match crate::runtime_context::initialize_rara_context(
                &crate::config::ConfigManager::new()
                    .and_then(|m| m.load())
                    .unwrap_or_default(),
                None,
            )
            .await
            {
                Ok(b) => b,
                Err(_e) => {
                    let _ = responder.respond(PromptResponse::new(StopReason::EndTurn));
                    return Ok(());
                }
            };
            *active_agent = Some(bootstrap.into_agent());
        }

        let agent = active_agent.as_mut().unwrap();
        agent.set_cancellation_token(Some(cancel_token.clone()));

        // Create the control envelope.
        let provenance = crate::runtime_control::RuntimeProvenance::protocol(
            crate::runtime_control::RuntimeControllerKind::Acp,
            "acp",
            Some(session_id.clone()),
            None,
        );
        let request = crate::runtime_control::RuntimeControlRequest::Input(
            crate::runtime_control::InputControlRequest::SubmitUserPrompt {
                prompt: prompt_text,
            },
        );
        let envelope = crate::runtime_control::RuntimeControlEnvelope {
            request_id: uuid::Uuid::new_v4().to_string(),
            provenance,
            request,
        };

        // Wire up the event reporter to send ACP notifications.
        let cx_for_report = cx.clone();
        let sid_for_report = req.session_id.clone();
        let bus = self.event_bus.clone();

        let mut on_event = move |control_event: crate::runtime_control::RuntimeControlEvent| {
            // Forward to RuntimeEventBus for other subscribers.
            let _ = bus.send_with_provenance(
                match &control_event.event {
                    crate::runtime_control::RuntimeEvent::Session(
                        crate::runtime_control::SessionEvent::Status { message },
                    ) => crate::agent::AgentEvent::Status(message.clone()),
                    crate::runtime_control::RuntimeEvent::Session(_) => {
                        return;
                    }
                    crate::runtime_control::RuntimeEvent::Assistant(ae) => match ae {
                        crate::runtime_control::AssistantEvent::TextDelta(text) => {
                            crate::agent::AgentEvent::AssistantDelta(text.clone())
                        }
                        crate::runtime_control::AssistantEvent::ThinkingDelta(text) => {
                            crate::agent::AgentEvent::AssistantThinkingDelta(text.clone())
                        }
                        _ => return,
                    },
                    crate::runtime_control::RuntimeEvent::Tool(te) => match te {
                        crate::runtime_control::ToolEvent::Use { name, input, .. } => {
                            crate::agent::AgentEvent::ToolUse {
                                name: name.clone(),
                                input: input.clone(),
                            }
                        }
                        crate::runtime_control::ToolEvent::Result {
                            name,
                            content,
                            is_error,
                        } => crate::agent::AgentEvent::ToolResult {
                            name: name.clone(),
                            content: content.clone(),
                            is_error: *is_error,
                        },
                        _ => return,
                    },
                    _ => return,
                },
                control_event.provenance.clone(),
            );

            // Translate to ACP SessionNotification.
            match control_event.event {
                crate::runtime_control::RuntimeEvent::Assistant(
                    crate::runtime_control::AssistantEvent::TextDelta(text),
                ) => {
                    let chunk = ContentChunk::new(AcpContentBlock::Text(TextContent::new(text)));
                    let _ = cx_for_report.send_notification(SessionNotification::new(
                        sid_for_report.clone(),
                        SessionUpdate::AgentMessageChunk(chunk),
                    ));
                }
                crate::runtime_control::RuntimeEvent::Tool(
                    crate::runtime_control::ToolEvent::Result { name, content, .. },
                ) => {
                    let label = format!("\n[Tool result: {name}]\n{}\n", content);
                    let chunk = ContentChunk::new(AcpContentBlock::Text(TextContent::new(label)));
                    let _ = cx_for_report.send_notification(SessionNotification::new(
                        sid_for_report.clone(),
                        SessionUpdate::AgentMessageChunk(chunk),
                    ));
                }
                _ => {}
            }
        };

        // Dispatch via control plane.
        let result = crate::control_plane::dispatch(
            envelope,
            &self.mcp_manager,
            &self.prompt_registry,
            &self.skill_registry,
            &self.memory_handler,
            &self.hook_registry,
            Some(agent),
            &mut on_event,
        )
        .await;

        let stop_reason = match result {
            Ok(()) => StopReason::EndTurn,
            Err(e) if e.contains("cancelled") => StopReason::Cancelled,
            Err(_) => StopReason::EndTurn, // ACP StopReason doesn't have Error, using EndTurn as fallback
        };

        responder.respond(PromptResponse::new(stop_reason))?;
        Ok(())
    }
}

/// Extract plain text from ACP content blocks.
fn extract_prompt_text(prompt: &[AcpContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| {
            if let AcpContentBlock::Text(text) = block {
                Some(text.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
