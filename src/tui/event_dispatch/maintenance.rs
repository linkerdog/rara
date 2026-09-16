use crate::agent::Agent;
use crate::tui::runtime_port::{RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand};
use crate::tui::state::TuiApp;

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
