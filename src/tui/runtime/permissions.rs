use std::sync::atomic::Ordering;

use crate::agent::Agent;
use crate::tui::permission_policy::{PermissionPreset, permission_preset};
use crate::tui::state::{PermissionMode, TuiApp};

#[cfg(test)]
#[path = "permissions_test.rs"]
mod tests;

pub(crate) fn request_permission_mode(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    mode: PermissionMode,
) {
    if permission_preset(mode).is_none() {
        app.push_notice("Custom describes the effective policy. Choose a named preset.");
        return;
    }
    if app.is_busy() {
        if mode == app.effective_permission_mode() {
            app.pending_permission_mode = None;
            app.push_notice(format!("Keeping current permissions: {}.", mode.label()));
        } else {
            app.pending_permission_mode = Some(mode);
            app.push_notice(format!(
                "Permissions pending: {}. Applies after the current task finishes.",
                mode.label()
            ));
        }
    } else {
        app.pending_permission_mode = None;
        apply_permission_mode(app, agent_slot, mode);
        app.push_notice(format!("Permissions applied: {}.", mode.label()));
    }
}

fn apply_permission_mode(app: &mut TuiApp, agent_slot: &mut Option<Agent>, mode: PermissionMode) {
    if let Some(preset) = permission_preset(mode) {
        apply_policy(app, agent_slot.as_mut(), preset);
    }
}

/// Apply only after interpreting the finished task under its original mode.
/// Callers must do this before starting any continuation with the returned agent.
pub(super) fn apply_pending_permission_mode(app: &mut TuiApp, agent: &mut Agent) -> bool {
    let Some(mode) = app.pending_permission_mode.take() else {
        return false;
    };
    let Some(preset) = permission_preset(mode) else {
        app.push_notice("Cannot apply a Custom permission preset.");
        return false;
    };
    apply_policy(app, Some(agent), preset);
    app.push_notice(format!("Permissions applied: {}.", mode.label()));
    true
}

fn apply_policy(app: &mut TuiApp, agent: Option<&mut Agent>, preset: &PermissionPreset) {
    app.permission_mode = preset.mode;
    app.set_agent_execution_mode(preset.execution);
    app.bash_approval_mode = preset.approval;
    app.sandbox_network_access
        .store(preset.network, Ordering::Relaxed);
    if let Some(agent) = agent {
        agent.set_execution_mode(preset.execution);
        agent.set_bash_approval_mode(preset.approval);
        agent.set_full_access_mode(preset.full_access);
    }
}
