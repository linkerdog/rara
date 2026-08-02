mod commands;
pub(crate) use commands::apply_permission_mode;
mod events;
pub(super) use events::apply_tui_event;
pub(super) mod tasks;

pub(super) fn emit_query_heartbeat(app: &mut TuiApp) -> bool {
    tasks::emit_query_heartbeat(app)
}

use std::sync::Arc;

use super::state::{LocalCommand, OAuthLoginMode, TuiApp};
use crate::agent::{Agent, BashApprovalDecision};
use crate::oauth::OAuthManager;

pub async fn execute_local_command(
    command: LocalCommand,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
) -> anyhow::Result<bool> {
    commands::execute_local_command(command, app, agent_slot, oauth_manager).await
}

pub fn start_query_task(app: &mut TuiApp, prompt: String, agent: Agent) {
    tasks::start_query_task(app, prompt, agent);
}

pub fn start_input_control_task(
    app: &mut TuiApp,
    agent: Agent,
    request: crate::runtime_control::InputControlRequest,
    notice: String,
    phase: super::state::RuntimePhase,
    phase_detail: Option<String>,
) {
    tasks::start_input_control_task(app, agent, request, notice, phase, phase_detail);
}

pub fn request_running_task_cancellation(app: &mut TuiApp) {
    tasks::request_running_task_cancellation(app);
}

pub fn start_pending_approval_task(
    app: &mut TuiApp,
    selection: BashApprovalDecision,
    agent: Agent,
) {
    tasks::start_pending_approval_task(app, selection, agent);
}

pub fn start_plan_approval_resume_task(
    app: &mut TuiApp,
    decision: crate::runtime_control::PlanApprovalDecision,
    feedback: Option<String>,
    agent: Agent,
) {
    tasks::start_plan_approval_resume_task(app, decision, feedback, agent);
}

pub fn start_rebuild_task(app: &mut TuiApp) {
    tasks::start_rebuild_task(app);
}

pub fn start_oauth_task(app: &mut TuiApp, oauth_manager: Arc<OAuthManager>, mode: OAuthLoginMode) {
    tasks::start_oauth_task(app, oauth_manager, mode);
}

pub fn start_deepseek_model_list_task(app: &mut TuiApp) {
    tasks::start_deepseek_model_list_task(app);
}

pub fn start_kimi_model_list_task(app: &mut TuiApp) {
    tasks::start_kimi_model_list_task(app);
}
