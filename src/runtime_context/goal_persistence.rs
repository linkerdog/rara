use std::sync::Arc;

use rara_state::state_db::StateDb;

use super::RuntimeBootstrapOptions;
use crate::runtime_goals::GoalHandle;
use crate::workspace::WorkspaceMemory;

pub(super) fn bind_bootstrap_goal(
    goal: &GoalHandle,
    options: &mut RuntimeBootstrapOptions,
    workspace: &WorkspaceMemory,
    warnings: &mut Vec<String>,
) {
    if !options.transcript_persistence {
        return;
    }
    let thread_id = options
        .session_id
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string());
    let restored = StateDb::new_for_root_dir(workspace.rara_dir.clone())
        .and_then(|db| goal.restore_for_thread(thread_id, Arc::new(db)));
    if let Err(error) = restored {
        let reason = format!("{error:#}");
        log::warn!("Goal persistence unavailable for thread {thread_id}: {reason}");
        goal.disable_after_persistence_failure(reason.clone());
        warnings.push(format!("Goal persistence unavailable: {reason}"));
    }
}

#[cfg(test)]
#[path = "goal_persistence_tests.rs"]
mod tests;
