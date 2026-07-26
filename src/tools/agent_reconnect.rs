use rara_persistence::thread_data::PersistedStructuredRolloutEvent;
use rara_persistence::thread_rollout_log;
use rara_tools::tool::ToolError;

use super::{BackgroundSubAgentRecord, SubagentProgress};
use crate::session::SessionManager;

pub(super) fn durable_subagent_record(
    session_manager: &SessionManager,
    parent_session_id: &str,
    agent_id: &str,
) -> Result<Option<BackgroundSubAgentRecord>, ToolError> {
    let records = durable_subagent_records(session_manager, parent_session_id)?;
    Ok(records
        .into_iter()
        .rev()
        .find(|record| record.agent_id == agent_id))
}

pub(super) fn durable_subagent_records(
    session_manager: &SessionManager,
    parent_session_id: &str,
) -> Result<Vec<BackgroundSubAgentRecord>, ToolError> {
    let events =
        thread_rollout_log::load_rollout_events(&session_manager.storage_dir, parent_session_id)
            .map_err(|err| {
                ToolError::ExecutionFailed(format!(
                    "failed to load sub-agent rollout events for {parent_session_id}: {err}"
                ))
            })?;
    let mut records = Vec::new();
    for event in events {
        let PersistedStructuredRolloutEvent::SpawnAgent {
            recorded_at,
            agent_id,
            name,
            child_session_id,
            status,
            summary,
            ..
        } = event
        else {
            continue;
        };
        let timestamp = recorded_at.and_then(|value| u64::try_from(value).ok());
        records.push(BackgroundSubAgentRecord {
            progress: SubagentProgress::new(name.clone().unwrap_or_else(|| "sub-agent".into())),
            kind: "reconnected",
            parent_session_id: Some(parent_session_id.to_string()),
            status,
            agent_id,
            session_id: child_session_id,
            name,
            provider: None,
            model: None,
            summary,
            error: None,
            persistence_error: None,
            plan: None,
            plan_explanation: None,
            request_user_input: None,
            started_at: timestamp.unwrap_or_default(),
            finished_at: timestamp,
        });
    }
    Ok(records)
}
