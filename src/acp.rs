//! ACP (Agent Client Protocol) integration for RARA.
//!
//! Implements an ACP-compliant agent with a full tool-calling loop.
//! The agent accepts stdio transports per the ACP spec.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_client_protocol::{
    Agent, Client, ConnectionTo, Dispatch, Responder, Result as AcpResult,
    schema::{
        AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification,
        ContentChunk, InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse,
        PromptRequest, PromptResponse, SessionId, SessionNotification, SessionUpdate, StopReason,
        TextContent,
    },
};
use rara_tools::tool::{ToolManager, ToolProgressEvent};
use serde_json::Value;
use tokio::io::{stdin, stdout};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::agent::Message;
use crate::llm::{ContentBlock, LlmBackend, LlmStreamEvent};
use crate::runtime_control::RuntimeControllerKind;
use crate::runtime_event_bus::RuntimeEventBus;

/// Maximum agentic turns per ACP prompt to prevent infinite loops.
const MAX_ACP_TURNS: usize = 15;

/// ACP agent state shared across request handlers.
pub struct RaraAcpAgent {
    pub llm_backend: Arc<dyn LlmBackend>,
    pub tool_manager: Arc<ToolManager>,
    pub event_bus: Arc<RuntimeEventBus>,
    /// Cancellation flag for the current prompt.
    cancel: AtomicBool,
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
            cancel: AtomicBool::new(false),
        }
    }

    fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn reset_cancel(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Run the ACP agent on stdio using the ACP builder API.
    pub async fn run_acp_stdio(self) -> AcpResult<()> {
        let llm = self.llm_backend.clone();
        let tools = self.tool_manager.clone();
        let this = Arc::new(self);

        let transport =
            agent_client_protocol::ByteStreams::new(stdout().compat_write(), stdin().compat());

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
                    let tools = tools.clone();
                    let this = this.clone();
                    async move |req: PromptRequest, responder, cx: ConnectionTo<Client>| {
                        this.handle_prompt(req, responder, cx, &*llm, &tools).await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                {
                    let this = this.clone();
                    async move |_notif: CancelNotification, _cx: ConnectionTo<Client>| {
                        eprintln!("[acp] cancel notification received");
                        this.request_cancel();
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
        llm: &dyn LlmBackend,
        tool_manager: &ToolManager,
    ) -> AcpResult<()> {
        let session_id = req.session_id.clone();
        let prompt_text = extract_prompt_text(&req.prompt);

        if prompt_text.trim().is_empty() {
            responder.respond(PromptResponse::new(StopReason::EndTurn))?;
            return Ok(());
        }

        self.reset_cancel();

        let tool_schemas = tool_manager.get_schemas();
        let bus = self.event_bus.clone();

        // Build initial messages: system + user.
        let mut messages = vec![
            Message {
                role: "system".to_string(),
                content: Value::String(rara_system_prompt()),
            },
            Message {
                role: "user".to_string(),
                content: Value::String(prompt_text),
            },
        ];

        for turn in 0..MAX_ACP_TURNS {
            if self.is_cancelled() {
                eprintln!("[acp] prompt cancelled at turn {turn}, session={session_id}");
                let err_chunk =
                    ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                        TextContent::new("\n\n[Cancelled]".to_string()),
                    ));
                let _ = cx.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentMessageChunk(err_chunk),
                ));
                break;
            }

            // -- Call LLM with streaming callback for text deltas --
            let cx_for_cb = cx.clone();
            let sid_for_cb = session_id.to_string();
            let bus_for_cb = bus.clone();

            let mut on_event = move |event: LlmStreamEvent| {
                if let LlmStreamEvent::TextDelta(text) = event {
                    let chunk =
                        ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                            TextContent::new(text.clone()),
                        ));
                    let _ = cx_for_cb.send_notification(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentMessageChunk(chunk),
                    ));
                    let _ = bus_for_cb.send_with_provenance(
                        crate::agent::AgentEvent::AssistantDelta(text),
                        crate::runtime_control::RuntimeProvenance {
                            controller: RuntimeControllerKind::Acp,
                            adapter: None,
                            session_id: Some(sid_for_cb.clone()),
                            source_id: None,
                            trust: crate::runtime_control::RuntimeSourceTrust::Trusted,
                            authorship: crate::runtime_control::RuntimeSourceAuthorship::Generated,
                        },
                    );
                }
            };

            let response = match llm
                .ask_streaming(&messages, &tool_schemas, &mut on_event)
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    eprintln!("[acp] LLM error at turn {turn}: {e:?}, session={session_id}");
                    let err_chunk =
                        ContentChunk::new(agent_client_protocol::schema::ContentBlock::Text(
                            TextContent::new(format!("\n\n[LLM error: {e}]")),
                        ));
                    let _ = cx.send_notification(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentMessageChunk(err_chunk),
                    ));
                    break;
                }
            };

            // -- Separate text and tool-use blocks from the LLM response --
            let text_blocks: Vec<ContentBlock> = response
                .content
                .iter()
                .filter(|b| matches!(b, ContentBlock::Text { .. }))
                .cloned()
                .collect();

            let tool_uses: Vec<&ContentBlock> = response
                .content
                .iter()
                .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
                .collect();

            // Push assistant message into conversation history.
            let assistant_blocks: Vec<Value> = response
                .content
                .iter()
                .map(content_block_to_value)
                .collect();

            if !assistant_blocks.is_empty() {
                messages.push(Message {
                    role: "assistant".to_string(),
                    content: Value::Array(assistant_blocks),
                });
            }

            // If no tool calls, we're done.
            if tool_uses.is_empty() {
                break;
            }

            // -- Execute each tool and collect results --
            let mut tool_results: Vec<Value> = Vec::new();
            for block in &tool_uses {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    eprintln!("[acp] turn {turn}: executing tool {name}, session={session_id}");

                    let result_text = match tool_manager.get_tool(name) {
                        Some(tool) => {
                            let mut reporter = |ev: ToolProgressEvent| {
                                eprintln!(
                                    "[acp] tool {name} progress: {:?}, session={session_id}",
                                    ev
                                );
                            };
                            match tool.call_with_events(input.clone(), &mut reporter).await {
                                Ok(output) => serde_json::to_string(&output).unwrap_or_default(),
                                Err(err) => format!("Tool error: {err}"),
                            }
                        }
                        None => format!("Unknown tool: {name}"),
                    };

                    // Emit tool result via EventBus and ACP.
                    let _ = bus.send_with_provenance(
                        crate::agent::AgentEvent::ToolResult {
                            name: name.clone(),
                            content: result_text.clone(),
                            is_error: false,
                        },
                        crate::runtime_control::RuntimeProvenance {
                            controller: RuntimeControllerKind::Acp,
                            adapter: None,
                            session_id: Some(session_id.to_string()),
                            source_id: None,
                            trust: crate::runtime_control::RuntimeSourceTrust::Trusted,
                            authorship: crate::runtime_control::RuntimeSourceAuthorship::Generated,
                        },
                    );

                    let label = format!("[Tool result: {name}]\n{result_text}\n");
                    let chunk = ContentChunk::new(
                        agent_client_protocol::schema::ContentBlock::Text(TextContent::new(label)),
                    );
                    let _ = cx.send_notification(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentMessageChunk(chunk),
                    ));

                    tool_results.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_call_id": id,
                        "content": [{"type": "text", "text": result_text}],
                    }));
                }
            }

            // Push tool results as a user message.
            messages.push(Message {
                role: "user".to_string(),
                content: Value::Array(tool_results),
            });
        }

        let stop_reason = if self.is_cancelled() {
            StopReason::Cancelled
        } else {
            StopReason::EndTurn
        };

        responder.respond(PromptResponse::new(stop_reason))?;
        Ok(())
    }
}

/// Extract plain text from ACP content blocks.
fn extract_prompt_text(prompt: &[agent_client_protocol::schema::ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| {
            if let agent_client_protocol::schema::ContentBlock::Text(text) = block {
                Some(text.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert a RARA ContentBlock to a JSON value for message serialization.
fn content_block_to_value(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text { text } => serde_json::json!({"type": "text", "text": text}),
        ContentBlock::ToolUse { id, name, input } => serde_json::json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        }),
        ContentBlock::ProviderMetadata { .. } => Value::Null,
    }
}

/// Build a concise system prompt for the ACP agent.
fn rara_system_prompt() -> String {
    "You are RARA, an AI coding agent running headless via ACP. \
     You have access to tools. Use them to complete the task. \
     Be concise and direct. When done, stop calling tools."
        .to_string()
}
