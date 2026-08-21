use crate::agent::Agent;
use crate::runtime_control::{InputControlRequest, ShellApprovalDecision};
use crate::tui::input_control;
use crate::tui::runtime_port::{RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand};
use crate::tui::state::{ActivePendingInteractionKind, PermissionMode, TuiApp};

pub(super) async fn request_maintenance(
    app: &mut TuiApp,
    agent_slot: &Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
    command: RuntimeMaintenanceCommand,
) -> anyhow::Result<()> {
    if let Some(runtime_port) = runtime_port {
        runtime_port
            .send(RuntimeCommand::Maintenance(command))
            .await?;
    } else {
        match command {
            RuntimeMaintenanceCommand::Rebuild => {
                crate::tui::runtime::start_rebuild_task_with_agent_tree_control(
                    app,
                    agent_slot.as_ref().and_then(Agent::agent_tree_control),
                )
            }
            RuntimeMaintenanceCommand::RefreshModelCatalog(provider) => {
                crate::tui::runtime::start_model_catalog_task(app, provider)
            }
            RuntimeMaintenanceCommand::Compact => {
                app.push_notice("Compaction requires an active runtime client.")
            }
        }
    }
    Ok(())
}

pub(super) async fn resume_pending_shell_approval_after_full_access(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<bool> {
    if app.permission_mode != PermissionMode::FullAccess
        || !app.active_pending_interaction().is_some_and(|interaction| {
            interaction.kind == ActivePendingInteractionKind::ShellApproval
        })
        || app.is_busy()
    {
        return Ok(false);
    }

    if let Some(runtime_port) = runtime_port {
        runtime_port
            .send(RuntimeCommand::Input(
                InputControlRequest::AnswerShellApproval {
                    decision: ShellApprovalDecision::Once,
                },
            ))
            .await?;
    } else if agent_slot.is_some() {
        input_control::answer_shell_approval(app, agent_slot, ShellApprovalDecision::Once);
    } else {
        app.push_notice("Permission mode: full-access. Approval is still preparing.");
    }
    Ok(true)
}
