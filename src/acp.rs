//! ACP (Agent Client Protocol) adapter with workspace-scoped session runtimes.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use agent_client_protocol::{
    Agent as AcpRoleAgent, Client, ConnectionTo, Dispatch, Responder, Result as AcpResult,
    schema::v1::{
        AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification,
        ContentBlock as AcpContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus,
        PromptRequest, PromptResponse, SessionId, SessionNotification, SessionUpdate, StopReason,
        TextContent, ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    },
};
use tokio::io::{stdin, stdout};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::agent::Agent;
use crate::config::RaraConfig;
use crate::hook_registry::HookRegistry;
use crate::mcp_connection_manager::McpConnectionManager;
use crate::protocol_sources::{MemoryControlHandler, PromptSourceRegistry, SkillSourceRegistry};
use crate::runtime_control::{
    RuntimeControlEnvelope, RuntimeControlRequest, RuntimeControllerKind, RuntimeProvenance,
};
use crate::runtime_event_bus::RuntimeEventBus;

struct AcpSessionRuntime {
    agent: Agent,
    event_bus: Arc<RuntimeEventBus>,
    mcp_manager: Arc<McpConnectionManager>,
    prompt_registry: Arc<PromptSourceRegistry>,
    skill_registry: Arc<SkillSourceRegistry>,
    hook_registry: Arc<HookRegistry>,
    memory_handler: Arc<MemoryControlHandler>,
}

struct AcpSession {
    cwd: PathBuf,
    runtime: tokio::sync::Mutex<Option<AcpSessionRuntime>>,
    cancellation_token: Arc<AtomicBool>,
}

/// ACP adapter state. Every ACP session owns an independent RARA runtime.
pub struct RaraAcpAgent {
    config: RaraConfig,
    plugin_dirs: Vec<PathBuf>,
    sessions: RwLock<HashMap<String, Arc<AcpSession>>>,
}

impl RaraAcpAgent {
    pub fn new(config: RaraConfig, plugin_dirs: Vec<PathBuf>) -> Self {
        Self {
            config,
            plugin_dirs,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn run_acp_stdio(self) -> AcpResult<()> {
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
                {
                    let this = this.clone();
                    move |req: NewSessionRequest,
                          responder: Responder<NewSessionResponse>,
                          _cx: ConnectionTo<Client>| {
                        let this = this.clone();
                        async move {
                            let session_id = SessionId::from(uuid::Uuid::new_v4().to_string());
                            this.sessions
                                .write()
                                .unwrap_or_else(|error| {
                                    log::warn!("ACP session registry was poisoned: {error}");
                                    error.into_inner()
                                })
                                .insert(
                                    session_id.to_string(),
                                    Arc::new(AcpSession {
                                        cwd: req.cwd,
                                        runtime: tokio::sync::Mutex::new(None),
                                        cancellation_token: Arc::new(AtomicBool::new(false)),
                                    }),
                                );
                            responder.respond(NewSessionResponse::new(session_id))
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let this = this.clone();
                    move |req: PromptRequest, responder, cx: ConnectionTo<Client>| {
                        let this = this.clone();
                        async move { this.handle_prompt(req, responder, cx).await }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                {
                    let this = this.clone();
                    async move |notification: CancelNotification, _cx: ConnectionTo<Client>| {
                        this.cancel_session(notification.session_id).await;
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

    fn session(&self, session_id: &SessionId) -> Option<Arc<AcpSession>> {
        self.sessions
            .read()
            .unwrap_or_else(|error| {
                log::warn!("ACP session registry was poisoned: {error}");
                error.into_inner()
            })
            .get(&session_id.to_string())
            .cloned()
    }

    async fn ensure_runtime(&self, session: &AcpSession) -> Result<AcpSessionRuntime, String> {
        let bootstrap = crate::runtime_context::initialize_rara_context_for_workspace_with_options(
            &self.config,
            Some(&session.cwd),
            None,
            crate::runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(
                self.plugin_dirs.clone(),
            ),
        )
        .await
        .map_err(|err| err.to_string())?;
        let event_bus = bootstrap.event_bus.clone();
        let (
            agent,
            _,
            _,
            _,
            _,
            mcp_manager,
            prompt_registry,
            skill_registry,
            hook_registry,
            _hook_runtime,
            _,
        ) = bootstrap.into_parts_with_runtime_extensions().await;
        Ok(AcpSessionRuntime {
            agent,
            event_bus: event_bus.clone(),
            mcp_manager,
            prompt_registry,
            skill_registry,
            hook_registry,
            memory_handler: Arc::new(MemoryControlHandler::new(event_bus)),
        })
    }

    async fn cancel_session(&self, session_id: SessionId) {
        let Some(session) = self.session(&session_id) else {
            return;
        };
        session.cancellation_token.store(true, Ordering::SeqCst);

        if let Ok(mut runtime) = session.runtime.try_lock()
            && let Some(runtime) = runtime.as_mut()
        {
            let event_bus = runtime.event_bus.clone();
            let envelope = cancel_envelope(session_id.clone());
            if let Err(err) = crate::control_plane::dispatch(
                envelope,
                &runtime.mcp_manager,
                &runtime.prompt_registry,
                &runtime.skill_registry,
                &runtime.memory_handler,
                &runtime.hook_registry,
                Some(&mut runtime.agent),
                move |event| {
                    event_bus.publish_control_event(event);
                },
            )
            .await
            {
                log::warn!("ACP cancellation failed for session {session_id}: {err}");
            }
        }
    }

    async fn handle_prompt(
        &self,
        req: PromptRequest,
        responder: Responder<PromptResponse>,
        cx: ConnectionTo<Client>,
    ) -> AcpResult<()> {
        let prompt = extract_prompt_text(&req.prompt);
        if prompt.trim().is_empty() {
            responder.respond(PromptResponse::new(StopReason::EndTurn))?;
            return Ok(());
        }
        let Some(session) = self.session(&req.session_id) else {
            responder.respond(PromptResponse::new(StopReason::EndTurn))?;
            return Ok(());
        };
        session.cancellation_token.store(false, Ordering::SeqCst);
        let mut runtime = session.runtime.lock().await;
        if runtime.is_none() {
            match self.ensure_runtime(&session).await {
                Ok(created) => *runtime = Some(created),
                Err(err) => {
                    log::warn!("ACP session initialization failed: {err}");
                    responder.respond(PromptResponse::new(StopReason::EndTurn))?;
                    return Ok(());
                }
            }
        }
        let runtime = runtime.as_mut().expect("runtime initialized");
        runtime
            .agent
            .set_cancellation_token(Some(session.cancellation_token.clone()));
        let provenance = RuntimeProvenance::protocol(
            RuntimeControllerKind::Acp,
            "acp",
            Some(req.session_id.to_string()),
            None,
        );
        let envelope = RuntimeControlEnvelope {
            request_id: uuid::Uuid::new_v4().to_string(),
            provenance: provenance.clone(),
            request: RuntimeControlRequest::Input(
                crate::runtime_control::InputControlRequest::SubmitUserPrompt { prompt },
            ),
        };
        let session_id = req.session_id.clone();
        let event_bus = runtime.event_bus.clone();
        let mut tool_ids = HashMap::<String, VecDeque<String>>::new();
        let mut sequence = 0_u64;
        let mut report = move |event: crate::runtime_control::RuntimeControlEvent| {
            sequence += 1;
            let event_id = format!("acp-{sequence:016x}");
            let _ = event_bus.publish_control_event(event.clone());
            match event.event {
                crate::runtime_control::RuntimeEvent::Assistant(
                    crate::runtime_control::AssistantEvent::TextDelta(text),
                ) => {
                    send_update(
                        &cx,
                        &session_id,
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(AcpContentBlock::Text(
                            TextContent::new(text),
                        ))),
                    );
                }
                crate::runtime_control::RuntimeEvent::Assistant(
                    crate::runtime_control::AssistantEvent::ThinkingDelta(text),
                ) => {
                    send_update(
                        &cx,
                        &session_id,
                        SessionUpdate::AgentThoughtChunk(ContentChunk::new(AcpContentBlock::Text(
                            TextContent::new(text),
                        ))),
                    );
                }
                crate::runtime_control::RuntimeEvent::Tool(
                    crate::runtime_control::ToolEvent::Use { name, input },
                ) => {
                    let id = format!("{event_id}-{name}");
                    tool_ids
                        .entry(name.clone())
                        .or_default()
                        .push_back(id.clone());
                    send_update(
                        &cx,
                        &session_id,
                        SessionUpdate::ToolCall(ToolCall::new(id, name).raw_input(input)),
                    );
                }
                crate::runtime_control::RuntimeEvent::Tool(
                    crate::runtime_control::ToolEvent::Result {
                        name,
                        content,
                        is_error,
                    },
                ) => {
                    if let Some(id) = tool_ids.get_mut(&name).and_then(VecDeque::pop_front) {
                        let status = if is_error {
                            ToolCallStatus::Failed
                        } else {
                            ToolCallStatus::Completed
                        };
                        send_update(
                            &cx,
                            &session_id,
                            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                id,
                                ToolCallUpdateFields::new().status(status).raw_output(
                                    serde_json::json!({
                                        "content": content,
                                        "is_error": is_error,
                                    }),
                                ),
                            )),
                        );
                    }
                }
                crate::runtime_control::RuntimeEvent::Tool(
                    crate::runtime_control::ToolEvent::Progress {
                        name,
                        stream,
                        chunk,
                    },
                ) => {
                    if let Some(id) = tool_ids.get(&name).and_then(|ids| ids.front()) {
                        send_update(
                            &cx,
                            &session_id,
                            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                id.clone(),
                                ToolCallUpdateFields::new().raw_output(serde_json::json!({
                                    "stream": stream,
                                    "chunk": chunk,
                                })),
                            )),
                        );
                    }
                }
                crate::runtime_control::RuntimeEvent::Plan(
                    crate::runtime_control::PlanEvent::Updated { steps, .. },
                ) => {
                    send_update(&cx, &session_id, acp_plan_update(steps));
                }
                _ => {}
            }
        };
        let result = crate::control_plane::dispatch(
            envelope,
            &runtime.mcp_manager,
            &runtime.prompt_registry,
            &runtime.skill_registry,
            &runtime.memory_handler,
            &runtime.hook_registry,
            Some(&mut runtime.agent),
            &mut report,
        )
        .await;
        responder.respond(PromptResponse::new(match result {
            Ok(()) => StopReason::EndTurn,
            Err(err) if err.contains("cancelled") => StopReason::Cancelled,
            Err(err) => {
                log::warn!("ACP prompt failed: {err}");
                StopReason::EndTurn
            }
        }))?;
        Ok(())
    }
}

fn cancel_envelope(session_id: SessionId) -> RuntimeControlEnvelope {
    RuntimeControlEnvelope {
        request_id: uuid::Uuid::new_v4().to_string(),
        provenance: RuntimeProvenance::protocol(
            RuntimeControllerKind::Acp,
            "acp",
            Some(session_id.to_string()),
            None,
        ),
        request: RuntimeControlRequest::Session(
            crate::runtime_control::SessionControlRequest::CancelCurrentTurn,
        ),
    }
}

fn acp_plan_update(steps: Vec<crate::runtime_control::PlanStepEvent>) -> SessionUpdate {
    let entries = steps
        .into_iter()
        .map(|step| {
            let status = match step.status {
                crate::runtime_control::PlanStepStatusEvent::Pending => PlanEntryStatus::Pending,
                crate::runtime_control::PlanStepStatusEvent::InProgress => {
                    PlanEntryStatus::InProgress
                }
                crate::runtime_control::PlanStepStatusEvent::Completed => {
                    PlanEntryStatus::Completed
                }
            };
            PlanEntry::new(step.step, PlanEntryPriority::Medium, status)
        })
        .collect();
    SessionUpdate::Plan(Plan::new(entries))
}

fn send_update(cx: &ConnectionTo<Client>, session_id: &SessionId, update: SessionUpdate) {
    if let Err(err) = cx.send_notification(SessionNotification::new(session_id.clone(), update)) {
        log::warn!("ACP session update delivery failed: {err}");
    }
}

fn extract_prompt_text(prompt: &[AcpContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            AcpContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sessions_keep_distinct_requested_workspaces() {
        let agent = RaraAcpAgent::new(RaraConfig::default(), Vec::new());
        let first = SessionId::from("first");
        let second = SessionId::from("second");
        agent.sessions.write().expect("session registry").insert(
            first.to_string(),
            Arc::new(AcpSession {
                cwd: PathBuf::from("/tmp/rara-acp-first"),
                runtime: tokio::sync::Mutex::new(None),
                cancellation_token: Arc::new(AtomicBool::new(false)),
            }),
        );
        agent.sessions.write().expect("session registry").insert(
            second.to_string(),
            Arc::new(AcpSession {
                cwd: PathBuf::from("/tmp/rara-acp-second"),
                runtime: tokio::sync::Mutex::new(None),
                cancellation_token: Arc::new(AtomicBool::new(false)),
            }),
        );

        assert_eq!(
            agent.session(&first).expect("first session").cwd,
            PathBuf::from("/tmp/rara-acp-first")
        );
        assert_eq!(
            agent.session(&second).expect("second session").cwd,
            PathBuf::from("/tmp/rara-acp-second")
        );
    }

    #[test]
    fn plan_events_translate_to_native_acp_updates() {
        let update = acp_plan_update(vec![crate::runtime_control::PlanStepEvent {
            step: "verify cancellation".to_string(),
            status: crate::runtime_control::PlanStepStatusEvent::Completed,
        }]);

        assert!(matches!(
            update,
            SessionUpdate::Plan(Plan { entries, .. })
                if entries.len() == 1
                && entries[0].content == "verify cancellation"
                && entries[0].status == PlanEntryStatus::Completed
        ));
    }

    #[test]
    fn cancellation_envelope_targets_the_notified_session() {
        let envelope = cancel_envelope(SessionId::from("second"));

        assert_eq!(envelope.provenance.session_id.as_deref(), Some("second"));
        assert!(matches!(
            envelope.request,
            RuntimeControlRequest::Session(
                crate::runtime_control::SessionControlRequest::CancelCurrentTurn
            )
        ));
    }

    #[test]
    fn cancellation_token_is_shared_without_waiting_for_runtime() {
        let session = AcpSession {
            cwd: PathBuf::from("/tmp/rara-acp-cancel"),
            runtime: tokio::sync::Mutex::new(None),
            cancellation_token: Arc::new(AtomicBool::new(false)),
        };

        let _runtime_guard = session.runtime.try_lock().expect("runtime lock");
        session.cancellation_token.store(true, Ordering::SeqCst);

        assert!(session.cancellation_token.load(Ordering::SeqCst));
    }
}
