mod builder;
include!("tasks/completion.rs");
mod oauth;
#[cfg(test)]
mod tests;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use builder::rebuild_agent_with_progress;
use rara_persistence::redaction::sanitize_url_for_display;
use rara_provider_catalog::{
    ModelCatalogProvider, ModelCatalogRequest, fallback_models, load_model_catalog,
};
use secrecy::ExposeSecret;
use tokio::sync::mpsc;

use super::super::state::{
    ListPickerKind, OAuthLoginMode, PermissionMode, RunningTask, RuntimePhase, SystemMessageKind,
    TaskCompletion, TaskKind, TuiApp, TuiEvent,
};
use super::events::{apply_tui_event, format_error_chain, runtime_event_from_agent_event};
use crate::agent::{Agent, AgentEvent, AgentOutputMode, BashApprovalDecision};
use crate::runtime_client::RuntimeTaskServices;
pub(crate) use crate::runtime_client::{goal_budget_limit_prompt, goal_continuation_prompt};
use crate::runtime_control::RuntimeProvenance;
use crate::runtime_event_bus::RuntimeEventBus;

fn local_tui_event_provenance(session_id: &str) -> RuntimeProvenance {
    RuntimeProvenance::local_tui(session_id.to_string())
}

fn classify_system_warning(warning: &str, default_kind: SystemMessageKind) -> SystemMessageKind {
    if warning.starts_with("embedding ·") || warning.starts_with("local embedding backend") {
        SystemMessageKind::EmbeddingStatus
    } else if warning.starts_with("Skill loading") {
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
    let event = match result {
        Ok(_) => AgentEvent::AgentStop {
            reason: "turn complete".to_string(),
        },
        Err(err) => {
            let message = format_error_chain(err);
            if message.contains("cancelled by user") {
                AgentEvent::AgentStop {
                    reason: "cancelled by user".to_string(),
                }
            } else {
                AgentEvent::AgentError {
                    message,
                    recoverable: false,
                }
            }
        }
    };
    forward_lifecycle_event_to_bus(bus, event, provenance);
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

fn try_start_queued_follow_up(
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

    app.bottom_pane.notice = Some("Running queued follow-up.".to_string());
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
    let bus = app.event_bus.clone().expect("event bus must exist");
    let event_provenance = local_tui_event_provenance(&agent.session_id);

    app.clear_pending_planning_suggestion();
    app.clear_active_live_sections();
    app.begin_running_turn();
    app.bottom_pane.notice = Some(notice);
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
    forward_lifecycle_event_to_bus(&bus, AgentEvent::AgentStart, &event_provenance);
    let handle = tokio::spawn(async move {
        let tx = sender.clone();
        let provenance =
            crate::runtime_control::RuntimeProvenance::local_tui(agent.session_id.clone());
        let envelope = crate::runtime_control::RuntimeControlEnvelope {
            request_id: uuid::Uuid::new_v4().to_string(),
            provenance,
            request: crate::runtime_control::RuntimeControlRequest::Input(request),
        };

        let bus_arg = Some(bus.clone());
        let lifecycle_bus = bus.clone();
        let lifecycle_provenance = event_provenance.clone();
        let result = crate::control_plane::dispatch(
            envelope,
            &mcp_manager,
            &prompt_registry,
            &skill_registry,
            &memory_handler,
            &hook_registry,
            Some(&mut agent),
            move |control_event| {
                bus_arg
                    .as_ref()
                    .expect("runtime event bus must exist")
                    .publish_control_event(control_event.clone());
                let _ = tx.send(TuiEvent::Runtime(Box::new(control_event)));
            },
        )
        .await;

        let result = result.map_err(|e| anyhow::anyhow!("{e}"));
        forward_task_result_lifecycle(&lifecycle_bus, &lifecycle_provenance, &result);
        TaskCompletion::Query { agent, result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: Some(cancellation_token),
        cancellation_requested: false,
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
    app.push_entry("You", prompt);
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

pub(super) fn start_compact_task(app: &mut TuiApp, mut agent: Agent) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let bus = app.event_bus.clone();
    let event_provenance = local_tui_event_provenance(&agent.session_id);
    agent.set_execution_mode(app.agent_execution_mode);
    agent.set_bash_approval_mode(app.bash_approval_mode);
    agent.set_full_access_mode(app.permission_mode == PermissionMode::FullAccess);
    app.bottom_pane.notice = Some("Compacting conversation history.".into());
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("compacting history".into()),
    );
    app.push_entry("You", "/compact");

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
        cancellation_requested: false,
    });
}

pub(super) fn start_review_task(app: &mut TuiApp, prompt: String, mut agent: Agent) {
    use crate::agent::{AgentExecutionMode, BashApprovalMode};
    let (sender, receiver) = mpsc::unbounded_channel();
    let bus = app.event_bus.clone();
    let event_provenance = local_tui_event_provenance(&agent.session_id);
    agent.set_execution_mode(AgentExecutionMode::Review);
    agent.set_bash_approval_mode(BashApprovalMode::Always);
    agent.set_full_access_mode(false);
    app.bottom_pane.notice = Some("Running code review.".into());
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("reviewing changes".into()),
    );
    app.push_entry("You", prompt.clone());

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
            .query_with_mode_and_events(prompt, AgentOutputMode::Silent, move |event| {
                forward_event_to_bus(&bus, &event, &event_provenance);
                let _ = tx.send(runtime_event_from_agent_event(
                    event,
                    event_provenance.clone(),
                ));
            })
            .await;
        forward_optional_task_result_lifecycle(&lifecycle_bus, &lifecycle_provenance, &result);
        TaskCompletion::Query { agent, result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
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
        app.permission_mode = PermissionMode::FullAccess;
        app.sandbox_network_access
            .store(true, std::sync::atomic::Ordering::Relaxed);
        app.set_agent_execution_mode(crate::agent::AgentExecutionMode::Execute);
        app.bash_approval_mode = crate::agent::BashApprovalMode::Always;
        agent.set_execution_mode(crate::agent::AgentExecutionMode::Execute);
        agent.set_bash_approval_mode(crate::agent::BashApprovalMode::Always);
        agent.set_full_access_mode(true);
    }

    let selection_label = match selection {
        BashApprovalDecision::Once => "run once",
        BashApprovalDecision::Prefix => "allow matching prefix",
        BashApprovalDecision::Always => "always allow bash",
        BashApprovalDecision::Suggestion => "suggestion only",
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

pub(super) fn start_rebuild_task(app: &mut TuiApp) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let config = app.config.clone();
    let plugin_dirs = app.explicit_plugin_dirs.clone();
    let provider = config.provider.clone();
    let model = config.model.clone().unwrap_or_else(|| "-".to_string());
    app.bottom_pane.notice = Some(format!("Rebuilding backend for {provider} / {model}."));
    app.set_runtime_phase(
        RuntimePhase::RebuildingBackend,
        Some(format!("preparing {provider} / {model}")),
    );
    app.push_entry("Download", format!("Preparing {} / {}", provider, model));

    let handle = tokio::spawn(async move {
        let tx = sender.clone();
        let progress: crate::local_backend::LocalProgressReporter = Arc::new(move |message| {
            let _ = tx.send(TuiEvent::Transcript {
                role: "Download",
                message,
            });
        });
        let result = rebuild_agent_with_progress(&config, Some(progress), plugin_dirs).await;
        TaskCompletion::Rebuild { result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Rebuild,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

pub(super) fn start_oauth_task(
    app: &mut TuiApp,
    oauth_manager: Arc<crate::oauth::OAuthManager>,
    mode: OAuthLoginMode,
) {
    oauth::start_oauth_task(app, oauth_manager, mode);
}

pub(super) fn start_deepseek_model_list_task(app: &mut TuiApp) {
    let (_sender, receiver) = mpsc::unbounded_channel();
    let api_key = app.config.api_key_secret();
    let surface = app.config.effective_provider_surface();
    let base_url = Some(
        surface
            .base_url
            .value
            .unwrap_or(crate::config::DEFAULT_DEEPSEEK_BASE_URL)
            .to_string(),
    );
    app.bottom_pane.notice = Some("Loading DeepSeek models.".into());
    app.set_runtime_phase(
        RuntimePhase::RebuildingBackend,
        Some("loading models".into()),
    );

    let handle = tokio::spawn(async move {
        let result = load_model_catalog(
            ModelCatalogProvider::DeepSeek,
            ModelCatalogRequest {
                api_key: api_key.as_ref(),
                base_url: base_url.as_deref(),
            },
        )
        .await
        .map(|catalog| catalog.models);
        TaskCompletion::DeepSeekModels { result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::DeepSeekModels,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

pub(super) fn start_kimi_model_list_task(app: &mut TuiApp) {
    let (_sender, receiver) = mpsc::unbounded_channel();
    let api_key = app.config.api_key_secret();
    let surface = app.config.effective_provider_surface();
    let base_url = Some(
        surface
            .base_url
            .value
            .unwrap_or(crate::config::DEFAULT_KIMI_BASE_URL)
            .to_string(),
    );
    app.bottom_pane.notice = Some("Loading Kimi models.".into());
    app.set_runtime_phase(
        RuntimePhase::RebuildingBackend,
        Some("loading models".into()),
    );

    let handle = tokio::spawn(async move {
        let result = load_model_catalog(
            ModelCatalogProvider::Kimi,
            ModelCatalogRequest {
                api_key: api_key.as_ref(),
                base_url: base_url.as_deref(),
            },
        )
        .await
        .map(|catalog| catalog.models);
        TaskCompletion::KimiModels { result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::KimiModels,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

pub(super) fn request_running_task_cancellation(app: &mut TuiApp) {
    let Some(task) = app.bottom_pane.running_task.as_mut() else {
        app.bottom_pane.notice = Some("No running task to cancel.".into());
        return;
    };
    if !matches!(task.kind, TaskKind::Query) {
        app.bottom_pane.notice =
            Some("Only running model queries can be cancelled from the TUI.".into());
        return;
    }
    if task.cancellation_requested {
        app.bottom_pane.notice =
            Some("Cancellation already requested. Waiting for the provider stream to stop.".into());
        return;
    }
    if let Some(token) = task.cancellation_token.as_ref() {
        token.store(true, Ordering::SeqCst);
        task.cancellation_requested = true;
        task.next_heartbeat_after_secs = 0;
        app.bottom_pane.notice = Some("Cancellation requested.".into());
        app.set_runtime_phase(
            RuntimePhase::ProcessingResponse,
            Some("cancelling query".into()),
        );
    } else {
        app.bottom_pane.notice = Some("This running task does not expose cancellation.".into());
    }
}
