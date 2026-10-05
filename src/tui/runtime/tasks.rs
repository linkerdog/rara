mod builder;
mod completion;
#[cfg(test)]
pub(crate) use completion::finish_running_task_if_ready;
pub(crate) use completion::{emit_query_heartbeat, finish_running_task_if_ready_from_runtime_port};
mod oauth;
#[cfg(test)]
mod tests;

use std::sync::{Arc, atomic::AtomicBool};
use std::time::Instant;

use builder::rebuild_agent_with_progress;
use rara_persistence::redaction::sanitize_url_for_display;
use rara_provider_catalog::{
    ModelCatalogProvider, ModelCatalogRequest, fallback_catalog, load_model_catalog,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::mpsc;

use super::super::state::{
    ListPickerKind, OAuthLoginMode, PermissionMode, RunningTask, RuntimePhase, SystemMessageKind,
    TaskCompletion, TaskKind, TuiApp, TuiEvent,
};
use super::events::{apply_tui_event, format_error_chain, runtime_event_from_agent_event};
use super::{QueryStopKind, QueryStopRequest, QueryTaskControl};
use crate::agent::{Agent, AgentEvent, AgentOutputMode, BashApprovalDecision};
use crate::config::RaraConfig;
use crate::runtime_client::RuntimeTaskServices;
pub(crate) use crate::runtime_client::{goal_budget_limit_prompt, goal_continuation_prompt};
use crate::runtime_control::RuntimeProvenance;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tui::message_role::MessageRole;
use crate::tui::state::NoticeLevel;

fn local_tui_event_provenance(session_id: &str) -> RuntimeProvenance {
    RuntimeProvenance::local_tui(session_id.to_string())
}

fn classify_system_warning(warning: &str, default_kind: SystemMessageKind) -> SystemMessageKind {
    if warning.starts_with("Skill loading") {
        SystemMessageKind::SkillLoading
    } else {
        default_kind
    }
}

/// Forward `event` to the broadcast bus when there are active subscribers.
/// Avoids the clone cost when nobody is listening (the common TUI-only case).
fn forward_event_to_bus(
    bus: &Option<Arc<RuntimeEventBus>>,
    event: &AgentEvent,
    provenance: &RuntimeProvenance,
) {
    if let Some(bus) = bus.as_ref()
        && bus.receiver_count() > 0
    {
        bus.send_with_provenance(event.clone(), provenance.clone());
    }
}

fn forward_lifecycle_event_to_bus(
    bus: &RuntimeEventBus,
    event: AgentEvent,
    provenance: &RuntimeProvenance,
) {
    if bus.receiver_count() > 0 {
        bus.send_with_provenance(event, provenance.clone());
    }
}

fn forward_task_result_lifecycle<T>(
    bus: &RuntimeEventBus,
    provenance: &RuntimeProvenance,
    result: &anyhow::Result<T>,
) {
    forward_lifecycle_event_to_bus(bus, task_result_lifecycle_event(result), provenance);
}

fn task_result_lifecycle_event<T>(result: &anyhow::Result<T>) -> AgentEvent {
    match result {
        Ok(_) => AgentEvent::AgentStop {
            reason: "turn complete".to_string(),
        },
        Err(err) => AgentEvent::AgentError {
            message: format_error_chain(err),
            recoverable: false,
        },
    }
}

fn forward_optional_task_result_lifecycle<T>(
    bus: &Option<Arc<RuntimeEventBus>>,
    provenance: &RuntimeProvenance,
    result: &anyhow::Result<T>,
) {
    if let Some(bus) = bus.as_ref() {
        forward_task_result_lifecycle(bus, provenance, result);
    }
}

fn forward_optional_lifecycle_event_to_bus(
    bus: &Option<Arc<RuntimeEventBus>>,
    event: AgentEvent,
    provenance: &RuntimeProvenance,
) {
    if let Some(bus) = bus.as_ref() {
        forward_lifecycle_event_to_bus(bus, event, provenance);
    }
}

fn merge_rebuilt_agent(rebuilt: Agent, previous: Agent) -> Agent {
    crate::runtime_client::RuntimeClient::merge_rebuilt_agent(rebuilt, previous)
}

pub(super) fn try_start_queued_follow_up(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    services: Option<RuntimeTaskServices>,
) {
    if app.bottom_pane.running_task.is_none() {
        app.release_pending_follow_ups();
    }
    if app.bottom_pane.running_task.is_some()
        || app.active_pending_interaction().is_some()
        || app.has_pending_planning_suggestion()
    {
        return;
    }

    let prompts = app.drain_queued_follow_up_messages();
    if prompts.is_empty() {
        return;
    }
    let prompt = prompts.join("\n\n");

    let Some(agent) = agent_slot.take() else {
        // If the agent is missing, re-queue the merged prompt
        app.queue_follow_up_message(prompt);
        return;
    };

    app.push_notice(NoticeLevel::Info, "Running queued follow-up.".to_string());
    if let Some(services) = services {
        start_query_task_with_services(app, prompt, agent, services);
    } else {
        start_query_task(app, prompt, agent);
    }
}

fn sync_bash_prefixes_from_config(app: &TuiApp, agent: &mut Agent) {
    let Ok(prefixes) = app.config_manager.load_allowed_command_prefixes() else {
        return;
    };
    for prefix in prefixes {
        if !agent.approved_bash_prefixes.contains(&prefix) {
            agent.approved_bash_prefixes.push(prefix);
        }
    }
}

#[cfg_attr(
    not(test),
    expect(
        clippy::panic,
        reason = "Production commands and completions must supply processor-owned services; only compatibility tests assemble services from TuiApp (runtime-task-service-ownership journal)."
    )
)]
fn legacy_task_services(app: &TuiApp) -> RuntimeTaskServices {
    #[cfg(test)]
    {
        RuntimeTaskServices {
            prompt_source_registry: app
                .prompt_source_registry
                .clone()
                .expect("prompt_registry must exist"),
            skill_source_registry: app
                .skill_source_registry
                .clone()
                .expect("skill_registry must exist"),
            hook_registry: app.hook_registry.clone().expect("hook_registry must exist"),
        }
    }
    #[cfg(not(test))]
    {
        let _ = app;
        panic!("runtime task services must be supplied by RuntimeCommandProcessor");
    }
}

pub(super) fn start_input_control_task(
    app: &mut TuiApp,
    agent: Agent,
    request: crate::runtime_control::InputControlRequest,
    notice: String,
    phase: RuntimePhase,
    phase_detail: Option<String>,
) {
    start_input_control_task_with_services(
        app,
        agent,
        request,
        notice,
        phase,
        phase_detail,
        legacy_task_services(app),
    );
}

#[cfg_attr(
    not(test),
    expect(
        clippy::expect_used,
        reason = "run_tui_session installs the session event bus, MCP manager, and memory handler before accepting input; compatibility fixtures install the same handles."
    )
)]
pub(crate) fn start_input_control_task_with_services(
    app: &mut TuiApp,
    agent: Agent,
    request: crate::runtime_control::InputControlRequest,
    notice: String,
    phase: RuntimePhase,
    phase_detail: Option<String>,
    services: RuntimeTaskServices,
) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let cancellation_token = Arc::new(AtomicBool::new(false));
    let query_control = QueryTaskControl::new(agent.session_id.clone());
    let task_control = query_control.clone();
    let task_control_for_app = query_control.clone();
    let bus = app.event_bus.clone().expect("event bus must exist");
    app.clear_pending_planning_suggestion();
    app.clear_active_live_sections();
    app.begin_running_turn();
    app.push_notice(NoticeLevel::Info, notice);
    app.set_runtime_phase(phase, phase_detail);

    let mcp_manager = app.mcp_manager.clone().expect("mcp_manager must exist");
    let prompt_registry = services.prompt_source_registry;
    let skill_registry = services.skill_source_registry;
    let memory_handler = app
        .memory_handler
        .clone()
        .expect("memory_handler must exist");
    let hook_registry = services.hook_registry;

    let mut agent = agent;
    agent.set_execution_mode(app.agent_execution_mode);
    agent.set_bash_approval_mode(app.bash_approval_mode);
    agent.set_full_access_mode(app.permission_mode == PermissionMode::FullAccess);
    sync_bash_prefixes_from_config(app, &mut agent);
    agent.set_cancellation_token(Some(cancellation_token.clone()));
    crate::tui::goal_resume::record_turn_started(app);
    let goal_turn = match &request {
        crate::runtime_control::InputControlRequest::AnswerPlanApproval { decision, .. } => {
            match decision {
                crate::runtime_control::PlanApprovalDecision::Approve => {
                    app.goal_handle.begin_turn(agent.total_input_tokens)
                }
                crate::runtime_control::PlanApprovalDecision::ContinuePlanning
                | crate::runtime_control::PlanApprovalDecision::Reject => None,
            }
        }
        crate::runtime_control::InputControlRequest::SubmitUserPrompt { .. }
        | crate::runtime_control::InputControlRequest::SubmitFollowUp { .. }
        | crate::runtime_control::InputControlRequest::AnswerPendingInput { .. }
        | crate::runtime_control::InputControlRequest::AnswerShellApproval { .. } => {
            match agent.execution_mode {
                crate::agent::AgentExecutionMode::Execute
                | crate::agent::AgentExecutionMode::Review => {
                    app.goal_handle.begin_turn(agent.total_input_tokens)
                }
                crate::agent::AgentExecutionMode::Plan => None,
            }
        }
    };
    bus.publish_raw(AgentEvent::AgentStart);
    let handle = tokio::spawn(async move {
        let tx = sender.clone();
        let task_bus = bus.clone();
        let envelope = crate::runtime_control::RuntimeControlEnvelope {
            request_id: uuid::Uuid::new_v4().to_string(),
            provenance: RuntimeProvenance::local_tui(agent.session_id.clone()),
            request: crate::runtime_control::RuntimeControlRequest::Input(request),
        };
        let mut pending_error = None;
        let result = crate::control_plane::dispatch(
            envelope,
            &mcp_manager,
            &prompt_registry,
            &skill_registry,
            &memory_handler,
            &hook_registry,
            Some(&mut agent),
            |event| {
                query_control.publish_dispatch_event(&bus, &tx, &mut pending_error, event);
            },
        )
        .await;
        let result = result.map_err(|error| anyhow::anyhow!(error));
        let result = task_control.publish_finished(&task_bus, &sender, result);
        TaskCompletion::Query {
            agent,
            result,
            goal_turn,
        }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: Some(cancellation_token),
        query_control: Some(task_control_for_app),
    });
}

pub(super) fn start_query_task(app: &mut TuiApp, prompt: String, agent: Agent) {
    start_query_task_with_services(app, prompt, agent, legacy_task_services(app));
}

pub(crate) fn start_query_task_with_services(
    app: &mut TuiApp,
    prompt: String,
    agent: Agent,
    services: RuntimeTaskServices,
) {
    let request = crate::runtime_control::InputControlRequest::SubmitUserPrompt {
        prompt: prompt.clone(),
    };
    app.push_entry(MessageRole::User, prompt);
    start_input_control_task_with_services(
        app,
        agent,
        request,
        "Running prompt.".into(),
        RuntimePhase::SendingPrompt,
        Some("sending prompt".into()),
        services,
    );
}

pub(super) fn start_goal_continuation_task(app: &mut TuiApp, prompt: String, agent: Agent) {
    start_goal_continuation_task_with_services(app, prompt, agent, legacy_task_services(app));
}

pub(crate) fn start_goal_continuation_task_with_services(
    app: &mut TuiApp,
    prompt: String,
    agent: Agent,
    services: RuntimeTaskServices,
) {
    let request = crate::runtime_control::InputControlRequest::SubmitUserPrompt { prompt };
    start_input_control_task_with_services(
        app,
        agent,
        request,
        "Continuing active goal.".into(),
        RuntimePhase::SendingPrompt,
        Some("continuing active goal".into()),
        services,
    );
}

pub(super) fn start_compact_task(app: &mut TuiApp, mut agent: Agent) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let bus = app.event_bus.clone();
    let event_provenance = local_tui_event_provenance(&agent.session_id);
    agent.set_execution_mode(app.agent_execution_mode);
    agent.set_bash_approval_mode(app.bash_approval_mode);
    agent.set_full_access_mode(app.permission_mode == PermissionMode::FullAccess);
    app.push_notice(NoticeLevel::Info, "Compacting conversation history.");
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("compacting history".into()),
    );
    app.push_entry(MessageRole::User, "/compact");

    let handle = tokio::spawn(async move {
        let tx = sender.clone();
        let lifecycle_bus = bus.clone();
        let lifecycle_provenance = event_provenance.clone();
        forward_optional_lifecycle_event_to_bus(
            &lifecycle_bus,
            AgentEvent::AgentStart,
            &lifecycle_provenance,
        );
        let result = agent
            .compact_now_with_reporter(move |event| {
                forward_event_to_bus(&bus, &event, &event_provenance);
                let _ = tx.send(runtime_event_from_agent_event(
                    event,
                    event_provenance.clone(),
                ));
            })
            .await;
        forward_optional_task_result_lifecycle(&lifecycle_bus, &lifecycle_provenance, &result);
        TaskCompletion::Compact { agent, result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Compact,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: None,
    });
}

#[cfg_attr(
    not(test),
    expect(
        clippy::expect_used,
        reason = "run_tui_session installs the session event bus before dispatching review commands; compatibility fixtures install the same handle."
    )
)]
pub(super) fn start_review_task(app: &mut TuiApp, prompt: String, mut agent: Agent) {
    use crate::agent::{AgentExecutionMode, BashApprovalMode};
    let (sender, receiver) = mpsc::unbounded_channel();
    let cancellation_token = Arc::new(AtomicBool::new(false));
    agent.set_cancellation_token(Some(cancellation_token.clone()));
    let bus = app.event_bus.clone().expect("event bus must exist");
    let query_control = QueryTaskControl::new(agent.session_id.clone());
    let task_control = query_control.clone();
    let task_control_for_app = query_control.clone();
    let event_provenance = local_tui_event_provenance(&agent.session_id);
    agent.set_execution_mode(AgentExecutionMode::Review);
    agent.set_bash_approval_mode(BashApprovalMode::Always);
    agent.set_full_access_mode(false);
    app.push_notice(NoticeLevel::Info, "Running code review.");
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("reviewing changes".into()),
    );
    app.push_entry(MessageRole::User, prompt.clone());
    crate::tui::goal_resume::record_turn_started(app);
    let goal_turn = app.goal_handle.begin_turn(agent.total_input_tokens);

    let handle = tokio::spawn(async move {
        let tx = sender.clone();
        let task_bus = bus.clone();
        bus.publish_raw(AgentEvent::AgentStart);
        query_control.publish_event(
            &bus,
            &sender,
            crate::runtime_control::wrap_agent_event(
                String::new(),
                0,
                event_provenance.clone(),
                AgentEvent::AgentStart,
            ),
        );
        let result = agent
            .query_with_mode_and_events(prompt, AgentOutputMode::Silent, move |event| {
                bus.publish_raw(event.clone());
                query_control.publish_event(
                    &bus,
                    &tx,
                    crate::runtime_control::wrap_agent_event(
                        uuid::Uuid::new_v4().to_string(),
                        0,
                        event_provenance.clone(),
                        event,
                    ),
                );
            })
            .await;
        let result = task_control.publish_finished(&task_bus, &sender, result);
        TaskCompletion::Query {
            agent,
            result,
            goal_turn,
        }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: Some(cancellation_token),
        query_control: Some(task_control_for_app),
    });
}

pub(super) fn start_pending_approval_task(
    app: &mut TuiApp,
    selection: BashApprovalDecision,
    agent: Agent,
) {
    start_pending_approval_task_with_services(app, selection, agent, legacy_task_services(app));
}

pub(crate) fn start_pending_approval_task_with_services(
    app: &mut TuiApp,
    selection: BashApprovalDecision,
    mut agent: Agent,
    services: RuntimeTaskServices,
) {
    if selection == BashApprovalDecision::Always {
        app.bash_approval_mode = crate::agent::BashApprovalMode::Always;
        if app.permission_mode != PermissionMode::FullAccess {
            app.permission_mode = PermissionMode::Custom;
        }
        agent.set_bash_approval_mode(crate::agent::BashApprovalMode::Always);
    }

    let selection_label = match selection {
        BashApprovalDecision::Once => "run once",
        BashApprovalDecision::Prefix => "allow matching prefix",
        BashApprovalDecision::Always => "allow for this session",
        BashApprovalDecision::Suggestion => "reject",
    };

    let request = crate::runtime_control::InputControlRequest::AnswerShellApproval {
        decision: selection.into(),
    };

    app.clear_pending_command_approval();
    start_input_control_task_with_services(
        app,
        agent,
        request,
        format!("Answering approval request: {selection_label}."),
        RuntimePhase::ProcessingResponse,
        Some("resuming after approval".into()),
        services,
    );
}

pub(super) fn start_plan_approval_resume_task(
    app: &mut TuiApp,
    decision: crate::runtime_control::PlanApprovalDecision,
    feedback: Option<String>,
    agent: Agent,
) {
    start_plan_approval_resume_task_with_services(
        app,
        decision,
        feedback,
        agent,
        legacy_task_services(app),
    );
}

pub(crate) fn start_plan_approval_resume_task_with_services(
    app: &mut TuiApp,
    decision: crate::runtime_control::PlanApprovalDecision,
    feedback: Option<String>,
    agent: Agent,
    services: RuntimeTaskServices,
) {
    let (notice, phase_detail) = match decision {
        crate::runtime_control::PlanApprovalDecision::Approve => (
            "Plan approved. Continuing with implementation.",
            "resuming approved plan",
        ),
        crate::runtime_control::PlanApprovalDecision::ContinuePlanning => {
            ("Continuing plan refinement.", "resuming plan refinement")
        }
        crate::runtime_control::PlanApprovalDecision::Reject => (
            "Plan rejected. Implementation cancelled.",
            "cancelling plan",
        ),
    };

    let request =
        crate::runtime_control::InputControlRequest::AnswerPlanApproval { decision, feedback };

    start_input_control_task_with_services(
        app,
        agent,
        request,
        notice.to_string(),
        RuntimePhase::ProcessingResponse,
        Some(phase_detail.into()),
        services,
    );
}

fn start_automatic_plan_implementation_task(
    app: &mut TuiApp,
    agent: Agent,
    services: Option<RuntimeTaskServices>,
) {
    let request = crate::runtime_control::InputControlRequest::AnswerPlanApproval {
        decision: crate::runtime_control::PlanApprovalDecision::Approve,
        feedback: None,
    };

    if let Some(services) = services {
        start_input_control_task_with_services(
            app,
            agent,
            request,
            "Plan generated automatically. Continuing with implementation.".into(),
            RuntimePhase::ProcessingResponse,
            Some("resuming approved plan".into()),
            services,
        );
    } else {
        start_input_control_task(
            app,
            agent,
            request,
            "Plan generated automatically. Continuing with implementation.".into(),
            RuntimePhase::ProcessingResponse,
            Some("resuming approved plan".into()),
        );
    }
}

pub(super) fn start_rebuild_task(
    app: &mut TuiApp,
    agent_tree_control: Option<Arc<crate::tools::agent::AgentTreeControl>>,
) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let config = app.config.clone();
    let plugin_dirs = app.explicit_plugin_dirs.clone();
    let provider = config.provider.clone();
    let model = config.model.clone().unwrap_or_else(|| "-".to_string());
    app.push_notice(
        NoticeLevel::Info,
        format!("Rebuilding backend for {provider} / {model}."),
    );
    app.set_runtime_phase(
        RuntimePhase::RebuildingBackend,
        Some(format!("preparing {provider} / {model}")),
    );
    app.push_entry(
        MessageRole::Download,
        format!("Preparing {} / {}", provider, model),
    );

    let handle = tokio::spawn(async move {
        let tx = sender.clone();
        let progress: crate::local_backend::LocalProgressReporter = Arc::new(move |message| {
            let _ = tx.send(TuiEvent::DownloadProgress(message));
        });
        let result =
            rebuild_agent_with_progress(&config, Some(progress), plugin_dirs, agent_tree_control)
                .await;
        TaskCompletion::Rebuild { result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Rebuild,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        query_control: None,
    });
}

pub(super) fn start_oauth_task(
    app: &mut TuiApp,
    oauth_manager: Arc<crate::oauth::OAuthManager>,
    mode: OAuthLoginMode,
) {
    oauth::start_oauth_task(app, oauth_manager, mode);
}

pub(super) fn start_model_catalog_task(app: &mut TuiApp, provider: ModelCatalogProvider) {
    let (_sender, receiver) = mpsc::unbounded_channel();
    let (api_key, base_url) = model_catalog_connection(&app.config, provider);
    let provider_label = match provider {
        ModelCatalogProvider::DeepSeek => "DeepSeek",
        ModelCatalogProvider::Kimi => "Moonshot AI",
    };
    app.push_notice(
        NoticeLevel::Info,
        format!("Loading {provider_label} models."),
    );
    app.set_runtime_phase(
        RuntimePhase::RebuildingBackend,
        Some("loading models".into()),
    );

    let handle = tokio::spawn(async move {
        let result = load_model_catalog(
            provider,
            ModelCatalogRequest {
                api_key: api_key.as_ref(),
                base_url: Some(base_url.as_str()),
            },
        )
        .await
        .map(|catalog| catalog.models);
        TaskCompletion::ModelCatalog { provider, result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::ModelCatalog,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        query_control: None,
    });
}

fn model_catalog_connection(
    config: &RaraConfig,
    provider: ModelCatalogProvider,
) -> (Option<SecretString>, String) {
    let (provider_id, default_base_url) = match provider {
        ModelCatalogProvider::DeepSeek => ("deepseek", crate::config::DEFAULT_DEEPSEEK_BASE_URL),
        ModelCatalogProvider::Kimi => ("kimi", crate::config::DEFAULT_KIMI_BASE_URL),
    };
    let mut provider_config = config.clone();
    provider_config.set_provider(provider_id);
    let api_key = provider_config.api_key_secret();
    let base_url = provider_config
        .effective_provider_surface()
        .base_url
        .value
        .unwrap_or(default_base_url)
        .to_string();
    (api_key, base_url)
}

pub(super) fn request_running_task_cancellation(app: &mut TuiApp, kind: QueryStopKind) -> bool {
    let Some(task) = app.bottom_pane.running_task.as_mut() else {
        app.push_notice(NoticeLevel::Info, "No running task to cancel.");
        return false;
    };
    if matches!(task.kind, TaskKind::ReviewPreparation) {
        let Some(token) = &task.cancellation_token else {
            log::warn!("Review preparation is missing its cancellation control");
            app.push_notice(
                NoticeLevel::Warning,
                "Review preparation cannot be cancelled right now.",
            );
            return false;
        };
        if token.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        task.handle.abort();
        app.push_notice(NoticeLevel::Info, "Stopping review preparation.");
        crate::tui::goal_resume::defer_for_user_stop(app);
        return true;
    }
    if !matches!(task.kind, TaskKind::Query) {
        app.push_notice(
            NoticeLevel::Warning,
            "Only running model queries can be cancelled from the TUI.",
        );
        return false;
    }
    if task.handle.is_finished() {
        app.push_notice(NoticeLevel::Info, "The query has already stopped.");
        crate::tui::goal_resume::defer_for_user_stop(app);
        return false;
    }
    if let Some((token, control)) = task
        .cancellation_token
        .as_ref()
        .zip(task.query_control.as_ref())
    {
        match control.request_stop(kind, token) {
            QueryStopRequest::AlreadyRequested => {
                app.push_notice(
                    NoticeLevel::Info,
                    "Stop already requested. Waiting for the provider stream to stop.",
                );
                return false;
            }
            QueryStopRequest::Finished => {
                app.push_notice(NoticeLevel::Info, "The query has already stopped.");
                crate::tui::goal_resume::defer_for_user_stop(app);
                return false;
            }
            QueryStopRequest::Requested => {}
        }
        task.next_heartbeat_after_secs = 0;
        let (notice, detail) = match kind {
            QueryStopKind::Cancel => ("Cancellation requested.", "cancelling query"),
            QueryStopKind::Interrupt => ("Interruption requested.", "interrupting query"),
        };
        app.push_notice(NoticeLevel::Info, notice);
        app.set_runtime_phase(RuntimePhase::ProcessingResponse, Some(detail.into()));
        crate::tui::goal_resume::defer_for_user_stop(app);
        true
    } else {
        app.push_notice(
            NoticeLevel::Warning,
            "This running task does not expose cancellation.",
        );
        false
    }
}
