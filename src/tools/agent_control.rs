use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rara_memory::memory_handle::MemoryHandle;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolCallContext, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use super::agent_reconnect::{durable_subagent_record, durable_subagent_records};
use super::{
    AgentDefinition, AgentDefinitionCache, PendingUserInput, PlanStep, PromptRuntimeConfig,
    SessionManager, SkillManager, SubAgentKind, SubAgentResult, SubagentBackendResolver,
    SubagentProgress, SubagentProviderTarget, WorkspaceMemory, run_sub_agent,
    serialize_pending_user_input, serialize_plan_steps,
};
use crate::llm::LlmBackend;

pub(crate) const DEFAULT_MAX_ACTIVE_SUBAGENTS: usize = 3;
pub(super) const BACKGROUND_SUBAGENT_COMPLETED_RETENTION: usize = 64;
const AGENT_MAILBOX_CAPACITY: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentResultDelivery {
    Direct,
    Mailbox,
}

pub(super) struct BackgroundSubAgentStart {
    pub(super) kind: SubAgentKind,
    pub(super) agent_id: String,
    pub(super) name: Option<String>,
    pub(super) definition: AgentDefinition,
    pub(super) model_target: Option<SubagentProviderTarget>,
    pub(super) parent_session_id: Option<String>,
    pub(super) instruction: String,
    pub(super) backend: Arc<dyn LlmBackend>,
    pub(super) backend_resolver: Arc<dyn SubagentBackendResolver>,
    pub(super) memory_handle: Arc<MemoryHandle>,
    pub(super) session_manager: Arc<SessionManager>,
    pub(super) workspace: Arc<WorkspaceMemory>,
    pub(super) prompt_config: PromptRuntimeConfig,
    pub(super) task_list_id: String,
    pub(super) agent_definitions: AgentDefinitionCache,
    pub(super) skill_manager: Option<Arc<RwLock<SkillManager>>>,
}

/// Limits shared execution resources for one root agent and its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentTreeConfig {
    max_active_subagents: NonZeroUsize,
}

impl AgentTreeConfig {
    /// Create a tree configuration with an explicit active-child capacity.
    pub fn new(max_active_subagents: NonZeroUsize) -> Self {
        Self {
            max_active_subagents,
        }
    }

    /// Return the maximum number of concurrently executing children.
    pub fn max_active_subagents(self) -> NonZeroUsize {
        self.max_active_subagents
    }
}

impl Default for AgentTreeConfig {
    fn default() -> Self {
        Self::new(
            NonZeroUsize::new(DEFAULT_MAX_ACTIVE_SUBAGENTS)
                .expect("default subagent capacity must be positive"),
        )
    }
}

/// One ordered message delivered through an agent session mailbox.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentMailboxMessage {
    pub sequence: u64,
    pub sender_agent_id: Option<String>,
    pub sender_path: String,
    pub kind: String,
    pub payload: String,
}

impl AgentMailboxMessage {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "sequence": self.sequence,
            "sender_agent_id": self.sender_agent_id,
            "sender_path": self.sender_path,
            "kind": self.kind,
            "payload": self.payload,
        })
    }
}

/// A bounded public projection of one child agent's lifecycle state.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentSnapshot {
    pub agent_id: String,
    pub path: String,
    pub session_id: String,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub kind: String,
    pub parent_session_id: Option<String>,
    pub status: String,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
}

/// Result of waiting for matching agent mailbox activity.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentWaitResult {
    pub messages: Vec<AgentMailboxMessage>,
    pub timed_out: bool,
}

#[derive(Clone, Debug)]
pub(super) struct BackgroundSubAgentRecord {
    pub(super) agent_id: String,
    pub(super) path: String,
    pub(super) session_id: String,
    pub(super) name: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) model: Option<String>,
    pub(super) progress: SubagentProgress,
    pub(super) kind: &'static str,
    pub(super) parent_session_id: Option<String>,
    pub(super) status: String,
    pub(super) summary: Option<String>,
    pub(super) error: Option<String>,
    pub(super) persistence_error: Option<String>,
    pub(super) plan: Option<Vec<PlanStep>>,
    pub(super) plan_explanation: Option<String>,
    pub(super) request_user_input: Option<PendingUserInput>,
    pub(super) started_at: u64,
    pub(super) finished_at: Option<u64>,
}

impl BackgroundSubAgentRecord {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "agent_id": self.agent_id,
            "path": self.path,
            "session_id": self.session_id,
            "name": self.name,
            "provider": self.provider,
            "model": self.model,
            "progress": {
                "tool_use_count": self.progress.tool_use_count,
                "tool_use_total": self.progress.tool_use_total,
                "latest_activity": self.progress.latest_activity(),
                "total_input_tokens": self.progress.total_input_tokens,
                "total_output_tokens": self.progress.total_output_tokens,
                "total_tokens": self.progress.total_tokens(),
            },
            "kind": self.kind,
            "parent_session_id": self.parent_session_id,
            "status": self.status,
            "summary": self.summary,
            "error": self.error,
            "persistence_error": self.persistence_error,
            "plan": self.plan.as_ref().map(|steps| serialize_plan_steps(steps)),
            "plan_explanation": self.plan_explanation,
            "request_user_input": self
                .request_user_input
                .as_ref()
                .map(serialize_pending_user_input),
            "started_at": self.started_at,
            "finished_at": self.finished_at,
        })
    }

    fn is_running(&self) -> bool {
        self.status == "running"
    }
}

impl From<&BackgroundSubAgentRecord> for AgentSnapshot {
    fn from(record: &BackgroundSubAgentRecord) -> Self {
        Self {
            agent_id: record.agent_id.clone(),
            path: record.path.clone(),
            session_id: record.session_id.clone(),
            name: record.name.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            kind: record.kind.to_string(),
            parent_session_id: record.parent_session_id.clone(),
            status: record.status.clone(),
            summary: record.summary.clone(),
            error: record.error.clone(),
            started_at: record.started_at,
            finished_at: record.finished_at,
        }
    }
}

/// Session-tree-owned lifecycle, capacity, and mailbox control.
pub struct AgentTreeControl {
    pub(super) inner: Arc<Mutex<AgentTreeState>>,
    active: Arc<Semaphore>,
    activity: Arc<Notify>,
    max_active_subagents: usize,
}

pub type BackgroundSubAgentStore = AgentTreeControl;

pub(super) struct AgentTreeState {
    pub(super) tasks: HashMap<String, BackgroundSubAgentRecord>,
    cancellations: HashMap<String, Arc<AtomicBool>>,
    mailboxes: HashMap<String, VecDeque<AgentMailboxMessage>>,
    next_sequence: u64,
}

impl Default for AgentTreeState {
    fn default() -> Self {
        Self {
            tasks: HashMap::new(),
            cancellations: HashMap::new(),
            mailboxes: HashMap::new(),
            next_sequence: 1,
        }
    }
}

impl Default for AgentTreeControl {
    fn default() -> Self {
        Self::new(AgentTreeConfig::default())
    }
}

impl AgentTreeControl {
    /// Create an isolated control plane for one root session tree.
    pub fn new(config: AgentTreeConfig) -> Self {
        let max_active_subagents = config.max_active_subagents().get();
        Self {
            inner: Arc::new(Mutex::new(AgentTreeState::default())),
            active: Arc::new(Semaphore::new(max_active_subagents)),
            activity: Arc::new(Notify::new()),
            max_active_subagents,
        }
    }

    /// Return the configured active-child capacity.
    pub fn max_active_subagents(&self) -> usize {
        self.max_active_subagents
    }

    #[cfg(test)]
    pub(super) fn available_permits(&self) -> usize {
        self.active.available_permits()
    }

    pub(super) fn start(
        self: &Arc<Self>,
        start: BackgroundSubAgentStart,
    ) -> Result<BackgroundSubAgentRecord, ToolError> {
        let permit = self.active.clone().try_acquire_owned().map_err(|_| {
            ToolError::ExecutionFailed(format!(
                "active sub-agent limit reached ({})",
                self.max_active_subagents
            ))
        })?;
        let (record, session_id, cancellation) = self.register(&start)?;
        let control = self.clone();
        let agent_id = start.agent_id.clone();
        tokio::spawn(async move {
            let result = control
                .execute_registered(start, session_id, cancellation, permit)
                .await;
            control.finish(&agent_id, &result, AgentResultDelivery::Mailbox);
        });
        Ok(record)
    }

    pub(super) async fn run(
        self: &Arc<Self>,
        start: BackgroundSubAgentStart,
    ) -> Result<SubAgentResult, ToolError> {
        let permit = self
            .active
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ToolError::ExecutionFailed("sub-agent control closed".into()))?;
        let (record, session_id, cancellation) = self.register(&start)?;
        let agent_id = record.agent_id;
        let result = self
            .execute_registered(start, session_id, cancellation, permit)
            .await;
        self.finish(&agent_id, &result, AgentResultDelivery::Direct);
        result
    }

    async fn execute_registered(
        self: &Arc<Self>,
        start: BackgroundSubAgentStart,
        session_id: String,
        cancellation: Arc<AtomicBool>,
        _permit: OwnedSemaphorePermit,
    ) -> Result<SubAgentResult, ToolError> {
        run_sub_agent(
            start.kind,
            &start.agent_id,
            Some(&start.definition),
            start.name.as_deref(),
            start.parent_session_id.as_deref(),
            &start.instruction,
            Some(session_id),
            Some(cancellation),
            start.model_target,
            start.backend,
            start.backend_resolver,
            start.memory_handle,
            start.session_manager,
            start.workspace,
            start.prompt_config,
            start.task_list_id,
            start.agent_definitions,
            start.skill_manager,
            Some(self.clone()),
        )
        .await
    }

    fn register(
        &self,
        start: &BackgroundSubAgentStart,
    ) -> Result<(BackgroundSubAgentRecord, String, Arc<AtomicBool>), ToolError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut inner = self.lock()?;
        if inner.tasks.contains_key(&start.agent_id) {
            return Err(ToolError::InvalidInput(format!(
                "duplicate sub-agent id: {}",
                start.agent_id
            )));
        }
        let path = child_path(&inner, start.parent_session_id.as_deref(), &start.agent_id);
        let record = BackgroundSubAgentRecord {
            agent_id: start.agent_id.clone(),
            path,
            session_id: session_id.clone(),
            name: start.name.clone(),
            provider: start
                .model_target
                .as_ref()
                .and_then(|target| target.provider.clone()),
            model: start
                .model_target
                .as_ref()
                .and_then(|target| target.model.clone()),
            progress: SubagentProgress::new(
                start.name.clone().unwrap_or_else(|| "sub-agent".into()),
            ),
            kind: start.kind.label(),
            parent_session_id: start.parent_session_id.clone(),
            status: "running".to_string(),
            summary: None,
            error: None,
            persistence_error: None,
            plan: None,
            plan_explanation: None,
            request_user_input: None,
            started_at: unix_timestamp_secs(),
            finished_at: None,
        };
        inner.tasks.insert(start.agent_id.clone(), record.clone());
        inner
            .cancellations
            .insert(start.agent_id.clone(), cancellation.clone());
        Ok((record, session_id, cancellation))
    }

    pub(super) fn record_model_resolution(
        &self,
        agent_id: &str,
        provider: &str,
        model: &str,
    ) -> Result<(), ToolError> {
        let mut inner = self.lock()?;
        let Some(record) = inner.tasks.get_mut(agent_id) else {
            return Ok(());
        };
        if record.is_running() {
            record.provider = Some(provider.to_string());
            record.model = Some(model.to_string());
        }
        Ok(())
    }

    pub(super) fn get_for_parent(
        &self,
        target: &str,
        parent_session_id: &str,
    ) -> Result<BackgroundSubAgentRecord, ToolError> {
        let inner = self.lock()?;
        resolve_record(&inner, target)
            .filter(|record| record.parent_session_id.as_deref() == Some(parent_session_id))
            .cloned()
            .ok_or_else(|| ToolError::InvalidInput(format!("unknown agent target: {target}")))
    }

    pub(crate) fn resolve_agent_id_for_parent(
        &self,
        target: &str,
        parent_session_id: &str,
    ) -> Result<String, ToolError> {
        Ok(self.get_for_parent(target, parent_session_id)?.agent_id)
    }

    #[cfg(test)]
    pub(super) fn list(&self) -> Result<Vec<BackgroundSubAgentRecord>, ToolError> {
        Ok(self.lock()?.tasks.values().cloned().collect())
    }

    pub(super) fn list_for_parent(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<BackgroundSubAgentRecord>, ToolError> {
        Ok(self
            .lock()?
            .tasks
            .values()
            .filter(|record| record.parent_session_id.as_deref() == Some(parent_session_id))
            .cloned()
            .collect())
    }

    pub(crate) fn snapshots_for_parent(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<AgentSnapshot>, ToolError> {
        Ok(self
            .list_for_parent(parent_session_id)?
            .iter()
            .map(AgentSnapshot::from)
            .collect())
    }

    pub(super) fn stop_for_parent(
        &self,
        target: &str,
        parent_session_id: &str,
    ) -> Result<BackgroundSubAgentRecord, ToolError> {
        self.stop_owned(target, Some(parent_session_id))
    }

    pub(crate) fn interrupt_for_parent(
        &self,
        target: &str,
        parent_session_id: &str,
    ) -> Result<AgentSnapshot, ToolError> {
        Ok(AgentSnapshot::from(
            &self.stop_for_parent(target, parent_session_id)?,
        ))
    }

    fn stop_owned(
        &self,
        target: &str,
        parent_session_id: Option<&str>,
    ) -> Result<BackgroundSubAgentRecord, ToolError> {
        let (stopped, token, mailbox_parent) = {
            let mut inner = self.lock()?;
            let agent_id = resolve_record(&inner, target)
                .filter(|record| {
                    parent_session_id
                        .map(|parent| record.parent_session_id.as_deref() == Some(parent))
                        .unwrap_or(true)
                })
                .map(|record| record.agent_id.clone())
                .ok_or_else(|| {
                    ToolError::InvalidInput(format!("unknown agent target: {target}"))
                })?;
            let record = inner.tasks.get_mut(&agent_id).expect("resolved record");
            if !record.is_running() {
                return Ok(record.clone());
            }
            record.status = "cancelled".to_string();
            record.finished_at = Some(unix_timestamp_secs());
            let stopped = record.clone();
            let token = inner.cancellations.remove(&agent_id);
            let mailbox_parent = stopped.parent_session_id.clone();
            if let Some(parent) = mailbox_parent.as_deref() {
                enqueue_completion(&mut inner, parent, &stopped);
            }
            prune_completed_subagents(&mut inner, Some(&agent_id));
            (stopped, token, mailbox_parent)
        };
        if let Some(token) = token {
            token.store(true, Ordering::SeqCst);
        }
        if mailbox_parent.is_some() {
            self.activity.notify_waiters();
        }
        Ok(stopped)
    }

    pub(super) fn finish(
        &self,
        agent_id: &str,
        result: &Result<SubAgentResult, ToolError>,
        delivery: AgentResultDelivery,
    ) {
        let mut notify = false;
        let Ok(mut inner) = self.inner.lock() else {
            log::warn!("sub-agent store poisoned while finishing {agent_id}");
            return;
        };
        inner.cancellations.remove(agent_id);
        let Some(record) = inner.tasks.get_mut(agent_id) else {
            return;
        };
        if !record.is_running() {
            prune_completed_subagents(&mut inner, Some(agent_id));
            return;
        }
        record.finished_at = Some(unix_timestamp_secs());
        match result {
            Ok(result) => {
                record.progress.total_input_tokens = result.total_input_tokens as usize;
                record.progress.total_output_tokens = result.total_output_tokens as usize;
                record.progress.total_cache_hit_tokens = result.total_cache_hit_tokens as usize;
                record.progress.total_cache_miss_tokens = result.total_cache_miss_tokens as usize;
                record.status = result.status.to_string();
                record.summary = Some(result.summary.clone());
                record.provider = Some(result.provider.clone());
                record.model = Some(result.model.clone());
                record.persistence_error = result.persistence_error.clone();
                record.plan = result.plan.clone();
                record.plan_explanation = result.plan_explanation.clone();
                record.request_user_input = result.request_user_input.clone();
                record.error = None;
            }
            Err(err) => {
                record.status = "failed".to_string();
                record.error = Some(err.to_string());
            }
        }
        let completed = record.clone();
        if matches!(delivery, AgentResultDelivery::Mailbox)
            && let Some(parent) = completed.parent_session_id.as_deref()
        {
            enqueue_completion(&mut inner, parent, &completed);
            notify = true;
        }
        prune_completed_subagents(&mut inner, Some(agent_id));
        drop(inner);
        if notify {
            self.activity.notify_waiters();
        }
    }

    pub(crate) fn drain_mailbox(&self, session_id: &str) -> Vec<AgentMailboxMessage> {
        self.drain_mailbox_matching(session_id, None)
            .unwrap_or_else(|err| {
                log::warn!("failed to drain agent mailbox for session {session_id}: {err}");
                Vec::new()
            })
    }

    #[cfg(test)]
    pub(crate) fn enqueue_test_message(
        &self,
        session_id: &str,
        sender_agent_id: Option<&str>,
        kind: &str,
        payload: &str,
    ) -> Result<(), ToolError> {
        {
            let mut inner = self.lock()?;
            let envelope = AgentMailboxMessage {
                sequence: next_sequence(&mut inner),
                sender_agent_id: sender_agent_id.map(str::to_string),
                sender_path: sender_agent_id
                    .map(|agent_id| format!("/root/{agent_id}"))
                    .unwrap_or_else(|| "/root".to_string()),
                kind: kind.to_string(),
                payload: payload.to_string(),
            };
            push_mailbox(&mut inner, session_id, envelope);
        }
        self.activity.notify_waiters();
        Ok(())
    }

    fn drain_mailbox_matching(
        &self,
        session_id: &str,
        targets: Option<&HashSet<String>>,
    ) -> Result<Vec<AgentMailboxMessage>, ToolError> {
        let mut inner = self.lock()?;
        let Some(mailbox) = inner.mailboxes.get_mut(session_id) else {
            return Ok(Vec::new());
        };
        if targets.is_none() {
            return Ok(mailbox.drain(..).collect());
        }
        let targets = targets.expect("checked above");
        let mut delivered = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(message) = mailbox.pop_front() {
            if message
                .sender_agent_id
                .as_ref()
                .is_some_and(|agent_id| targets.contains(agent_id))
            {
                delivered.push(message);
            } else {
                retained.push_back(message);
            }
        }
        *mailbox = retained;
        Ok(delivered)
    }

    pub(crate) async fn wait_for_messages(
        &self,
        session_id: &str,
        targets: Option<&HashSet<String>>,
        timeout: Duration,
    ) -> Result<(Vec<AgentMailboxMessage>, bool), ToolError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let messages = self.drain_mailbox_matching(session_id, targets)?;
            if !messages.is_empty() {
                return Ok((messages, false));
            }

            let notified = self.activity.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();

            let messages = self.drain_mailbox_matching(session_id, targets)?;
            if !messages.is_empty() {
                return Ok((messages, false));
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                || tokio::time::timeout(remaining, notified.as_mut())
                    .await
                    .is_err()
            {
                return Ok((Vec::new(), true));
            }
        }
    }

    pub(crate) fn send_to_child(
        &self,
        parent_session_id: &str,
        target: &str,
        kind: &str,
        payload: String,
    ) -> Result<AgentMailboxMessage, ToolError> {
        let envelope = {
            let mut inner = self.lock()?;
            let record = resolve_record(&inner, target)
                .filter(|record| {
                    record.parent_session_id.as_deref() == Some(parent_session_id)
                        && record.is_running()
                })
                .cloned()
                .ok_or_else(|| {
                    ToolError::InvalidInput(format!(
                        "agent target is unknown, not owned by this session, or already terminal: {target}"
                    ))
                })?;
            let envelope = AgentMailboxMessage {
                sequence: next_sequence(&mut inner),
                sender_agent_id: None,
                sender_path: "/root".to_string(),
                kind: kind.to_string(),
                payload,
            };
            push_mailbox(&mut inner, &record.session_id, envelope.clone());
            envelope
        };
        self.activity.notify_waiters();
        Ok(envelope)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, AgentTreeState>, ToolError> {
        self.inner
            .lock()
            .map_err(|_| ToolError::ExecutionFailed("sub-agent store poisoned".into()))
    }
}

fn child_path(inner: &AgentTreeState, parent_session_id: Option<&str>, agent_id: &str) -> String {
    let parent_path = parent_session_id
        .and_then(|session_id| {
            inner
                .tasks
                .values()
                .find(|record| record.session_id == session_id)
                .map(|record| record.path.as_str())
        })
        .unwrap_or("/root");
    format!("{parent_path}/{agent_id}")
}

fn resolve_record<'a>(
    inner: &'a AgentTreeState,
    target: &str,
) -> Option<&'a BackgroundSubAgentRecord> {
    inner
        .tasks
        .get(target)
        .or_else(|| inner.tasks.values().find(|record| record.path == target))
}

fn enqueue_completion(
    inner: &mut AgentTreeState,
    parent_session_id: &str,
    record: &BackgroundSubAgentRecord,
) {
    let payload = serde_json::to_string(&record.to_json())
        .unwrap_or_else(|_| format!("agent {} finished with {}", record.agent_id, record.status));
    let envelope = AgentMailboxMessage {
        sequence: next_sequence(inner),
        sender_agent_id: Some(record.agent_id.clone()),
        sender_path: record.path.clone(),
        kind: "completion".to_string(),
        payload,
    };
    push_mailbox(inner, parent_session_id, envelope);
}

fn next_sequence(inner: &mut AgentTreeState) -> u64 {
    let sequence = inner.next_sequence;
    inner.next_sequence = inner.next_sequence.saturating_add(1);
    sequence
}

fn push_mailbox(inner: &mut AgentTreeState, session_id: &str, envelope: AgentMailboxMessage) {
    let mailbox = inner.mailboxes.entry(session_id.to_string()).or_default();
    if mailbox.len() == AGENT_MAILBOX_CAPACITY {
        mailbox.pop_front();
        log::warn!("agent mailbox for session {session_id} reached capacity; dropped oldest item");
    }
    mailbox.push_back(envelope);
}

fn prune_completed_subagents(inner: &mut AgentTreeState, preserve_agent_id: Option<&str>) {
    let completed_count = inner
        .tasks
        .values()
        .filter(|record| record.finished_at.is_some())
        .count();
    if completed_count <= BACKGROUND_SUBAGENT_COMPLETED_RETENTION {
        return;
    }
    let mut candidates = inner
        .tasks
        .values()
        .filter(|record| {
            record.finished_at.is_some() && Some(record.agent_id.as_str()) != preserve_agent_id
        })
        .map(|record| {
            (
                record.agent_id.clone(),
                record.finished_at.unwrap_or(u64::MAX),
                record.started_at,
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.1.cmp(&right.1).then(left.2.cmp(&right.2)));
    let remove_count = completed_count.saturating_sub(BACKGROUND_SUBAGENT_COMPLETED_RETENTION);
    for (agent_id, _, _) in candidates.into_iter().take(remove_count) {
        inner.tasks.remove(&agent_id);
        inner.cancellations.remove(&agent_id);
    }
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "agent_control_test.rs"]
mod tests;
