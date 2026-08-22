//! ACP (Agent Client Protocol) adapter with workspace-scoped session runtimes.

use std::collections::HashMap;
use std::path::PathBuf;
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

use crate::agent::{AgentEvent, AgentOutputMode};
use crate::config::RaraConfig;
use crate::runtime_session::{RuntimeHost, RuntimeSession, RuntimeSessionError};

struct AcpSession {
    id: String,
    cwd: PathBuf,
    runtime: tokio::sync::Mutex<Option<RuntimeSession>>,
}

/// ACP adapter state. Every ACP session owns an independent RARA runtime.
pub struct RaraAcpAgent {
    config: RaraConfig,
    plugin_dirs: Vec<PathBuf>,
    sessions: RwLock<HashMap<String, Arc<AcpSession>>>,
    runtime_host: RuntimeHost,
}

impl RaraAcpAgent {
    pub fn new(config: RaraConfig, plugin_dirs: Vec<PathBuf>) -> Self {
        Self {
            config,
            plugin_dirs,
            sessions: RwLock::new(HashMap::new()),
            runtime_host: RuntimeHost::new(),
        }
    }

    pub async fn run_acp_stdio(self) -> AcpResult<()> {
        let this = Arc::new(self);
        let transport =
            agent_client_protocol::ByteStreams::new(stdout().compat_write(), stdin().compat());

        let result = AcpRoleAgent::builder(AcpRoleAgent)
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
                                        id: session_id.to_string(),
                                        cwd: req.cwd,
                                        runtime: tokio::sync::Mutex::new(None),
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
                async move |message: Dispatch, _cx: ConnectionTo<Client>| match message {
                    Dispatch::Request(_, responder) => responder.respond_with_error(
                        agent_client_protocol::util::internal_error("unhandled ACP message"),
                    ),
                    Dispatch::Notification(_) | Dispatch::Response(_, _) => Ok(()),
                },
                agent_client_protocol::on_receive_dispatch!(),
            )
            .connect_to(transport)
            .await;
        if let Err(err) = this.runtime_host.shutdown().await {
            log::warn!("ACP runtime host shutdown failed: {err}");
        }
        result
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

    fn runtime_options(
        &self,
        session: &AcpSession,
    ) -> crate::runtime_context::RuntimeBootstrapOptions {
        crate::runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(self.plugin_dirs.clone())
            .with_session_id(Some(session.id.clone()))
    }

    async fn ensure_runtime(&self, session: &AcpSession) -> Result<RuntimeSession, String> {
        let bootstrap = crate::runtime_context::initialize_rara_context_for_workspace_with_options(
            &self.config,
            Some(&session.cwd),
            None,
            self.runtime_options(session),
        )
        .await
        .map_err(|err| err.to_string())?;
        let runtime = RuntimeSession::from_bootstrap(bootstrap)
            .await
            .map_err(|err| err.to_string())?;
        self.runtime_host
            .insert(runtime.clone())
            .await
            .map_err(|err| err.to_string())?;
        Ok(runtime)
    }

    async fn cancel_session(&self, session_id: SessionId) {
        let Some(session) = self.session(&session_id) else {
            return;
        };
        let runtime = session.runtime.lock().await.clone();
        if let Some(runtime) = runtime
            && let Err(err) = runtime.cancel().await
            && !matches!(
                err,
                RuntimeSessionError::NotRunning | RuntimeSessionError::Closed
            )
        {
            log::warn!("ACP cancellation failed for session {session_id}: {err}");
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
        let runtime_handle = runtime.as_ref().expect("runtime initialized").clone();
        drop(runtime);
        let session_id = req.session_id.clone();
        let mut saw_text_delta = false;
        let mut report = move |event: AgentEvent| match event {
            AgentEvent::AssistantDelta(text) => {
                saw_text_delta = true;
                send_update(
                    &cx,
                    &session_id,
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(AcpContentBlock::Text(
                        TextContent::new(text),
                    ))),
                );
            }
            AgentEvent::AssistantText(text) if !saw_text_delta => {
                send_update(
                    &cx,
                    &session_id,
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(AcpContentBlock::Text(
                        TextContent::new(text),
                    ))),
                );
            }
            AgentEvent::AssistantText(_) => {}
            AgentEvent::AssistantThinkingDelta(text) => {
                send_update(
                    &cx,
                    &session_id,
                    SessionUpdate::AgentThoughtChunk(ContentChunk::new(AcpContentBlock::Text(
                        TextContent::new(text),
                    ))),
                );
            }
            AgentEvent::ToolUse {
                call_id,
                name,
                input,
            } => {
                send_update(
                    &cx,
                    &session_id,
                    SessionUpdate::ToolCall(ToolCall::new(call_id, name).raw_input(input)),
                );
            }
            AgentEvent::ToolResult {
                call_id,
                content,
                is_error,
                ..
            } => {
                let status = if is_error {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                };
                send_update(
                    &cx,
                    &session_id,
                    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                        call_id,
                        ToolCallUpdateFields::new()
                            .status(status)
                            .raw_output(serde_json::json!({
                                "content": content,
                                "is_error": is_error,
                            })),
                    )),
                );
            }
            AgentEvent::ToolProgress {
                call_id,
                stream,
                chunk,
                ..
            } => {
                send_update(
                    &cx,
                    &session_id,
                    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                        call_id,
                        ToolCallUpdateFields::new().raw_output(serde_json::json!({
                            "stream": crate::runtime_control::ToolStream::from(stream),
                            "chunk": chunk,
                        })),
                    )),
                );
            }
            AgentEvent::PlanUpdated { steps, .. } => {
                send_update(
                    &cx,
                    &session_id,
                    acp_plan_update(steps.into_iter().map(Into::into).collect()),
                );
            }
            _ => {}
        };
        let result = runtime_handle
            .query_with_events(prompt, AgentOutputMode::Silent, &mut report)
            .await;
        responder.respond(PromptResponse::new(match result {
            Ok(_) => StopReason::EndTurn,
            Err(RuntimeSessionError::Cancelled { .. }) => StopReason::Cancelled,
            Err(err) => {
                log::warn!("ACP prompt failed: {err}");
                StopReason::EndTurn
            }
        }))?;
        Ok(())
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
                id: first.to_string(),
                cwd: PathBuf::from("/tmp/rara-acp-first"),
                runtime: tokio::sync::Mutex::new(None),
            }),
        );
        agent.sessions.write().expect("session registry").insert(
            second.to_string(),
            Arc::new(AcpSession {
                id: second.to_string(),
                cwd: PathBuf::from("/tmp/rara-acp-second"),
                runtime: tokio::sync::Mutex::new(None),
            }),
        );

        assert_eq!(
            agent.session(&first).expect("first session").cwd,
            PathBuf::from("/tmp/rara-acp-first")
        );
        assert_eq!(agent.session(&first).expect("first session").id, "first");
        assert_eq!(
            agent.session(&second).expect("second session").cwd,
            PathBuf::from("/tmp/rara-acp-second")
        );
        assert_eq!(agent.session(&second).expect("second session").id, "second");
        assert_eq!(
            agent
                .runtime_options(&agent.session(&second).expect("second session"))
                .session_id
                .as_deref(),
            Some("second")
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
}
