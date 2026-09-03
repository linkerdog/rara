use std::collections::HashSet;

use rara_persistence::redaction::redact_secrets;
use rara_tools::tool::ToolError;

use super::{AgentTreeControl, BackgroundSubAgentRecord};
use crate::agent::AgentEvent;

/// Immutable child-agent activity projected into presentation surfaces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentActivitySnapshot {
    pub(crate) agent_id: String,
    pub(crate) path: String,
    pub(crate) name: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) summary: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) tool_use_count: usize,
    pub(crate) total_tokens: usize,
    pub(crate) latest_activity: Option<String>,
    pub(crate) started_at: u64,
}

impl From<&BackgroundSubAgentRecord> for AgentActivitySnapshot {
    fn from(record: &BackgroundSubAgentRecord) -> Self {
        Self {
            agent_id: record.agent_id.clone(),
            path: record.path.clone(),
            name: record.name.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            kind: record.kind.to_string(),
            status: record.status.clone(),
            summary: record.summary.clone(),
            error: record.error.clone(),
            tool_use_count: record.progress.tool_use_count,
            total_tokens: record.progress.total_tokens(),
            latest_activity: record.progress.latest_activity().map(str::to_string),
            started_at: record.started_at,
        }
    }
}

impl AgentTreeControl {
    pub(super) fn record_progress_event(
        &self,
        agent_id: &str,
        event: &AgentEvent,
    ) -> Result<(), ToolError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ToolError::ExecutionFailed("sub-agent store poisoned".into()))?;
        let Some(record) = inner.tasks.get_mut(agent_id) else {
            return Ok(());
        };
        if record.status != "running" {
            return Ok(());
        }

        match event {
            AgentEvent::Status(message) => record_progress_activity(record, message),
            AgentEvent::ToolUse { name, .. } => {
                record.progress.tool_use_count = record.progress.tool_use_count.saturating_add(1);
                record_progress_activity(record, &format!("Using {name}"));
            }
            AgentEvent::ToolResult { name, is_error, .. } => {
                let outcome = if *is_error { "failed" } else { "completed" };
                record_progress_activity(record, &format!("{name} {outcome}"));
            }
            AgentEvent::MemoryAction { message } => record_progress_activity(record, message),
            AgentEvent::AgentStart => record_progress_activity(record, "Starting"),
            AgentEvent::AgentStop { reason } => record_progress_activity(record, reason),
            AgentEvent::AgentError { message, .. } => record_progress_activity(record, message),
            AgentEvent::ModelRequest { input_tokens, .. } => {
                record.progress.total_input_tokens = record
                    .progress
                    .total_input_tokens
                    .saturating_add(*input_tokens as usize);
            }
            AgentEvent::ModelResponse { output_tokens, .. } => {
                record.progress.total_output_tokens = record
                    .progress
                    .total_output_tokens
                    .saturating_add(*output_tokens as usize);
            }
            AgentEvent::AssistantText(_)
            | AgentEvent::AssistantDelta(_)
            | AgentEvent::AssistantThinkingDelta(_)
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::McpStatusUpdated(_)
            | AgentEvent::McpStatusLoadFailed { .. }
            | AgentEvent::TodoUpdated(_)
            | AgentEvent::PlanUpdated { .. }
            | AgentEvent::ApprovalRequested { .. }
            | AgentEvent::ApprovalAnswered { .. }
            | AgentEvent::Compaction { .. } => {}
        }
        Ok(())
    }

    pub(crate) fn activity_snapshots_for_root(
        &self,
        root_session_id: &str,
    ) -> Result<Vec<AgentActivitySnapshot>, ToolError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| ToolError::ExecutionFailed("sub-agent store poisoned".into()))?;
        let mut visible_sessions = HashSet::from([root_session_id.to_string()]);
        let mut visible_agent_ids = HashSet::new();
        loop {
            let mut discovered = false;
            for record in inner.tasks.values() {
                if record
                    .parent_session_id
                    .as_ref()
                    .is_some_and(|parent| visible_sessions.contains(parent))
                    && visible_agent_ids.insert(record.agent_id.clone())
                {
                    visible_sessions.insert(record.session_id.clone());
                    discovered = true;
                }
            }
            if !discovered {
                break;
            }
        }
        let mut snapshots = inner
            .tasks
            .values()
            .filter(|record| visible_agent_ids.contains(&record.agent_id))
            .map(AgentActivitySnapshot::from)
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| {
            let left_rank = usize::from(left.status != "running");
            let right_rank = usize::from(right.status != "running");
            left_rank
                .cmp(&right_rank)
                .then(right.started_at.cmp(&left.started_at))
                .then(left.path.cmp(&right.path))
        });
        Ok(snapshots)
    }
}

fn record_progress_activity(record: &mut BackgroundSubAgentRecord, activity: &str) {
    let sanitized = redact_secrets(activity.to_string());
    let normalized = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return;
    }
    let mut bounded = normalized.chars().take(120).collect::<String>();
    if normalized.chars().count() > 120 {
        bounded.push('…');
    }
    record.progress.record_activity(bounded);
}
