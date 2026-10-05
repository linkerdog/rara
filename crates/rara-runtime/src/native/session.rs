use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use rara_core::{llm::backend::LlmBackend, llm::types::Message, tool::ToolManager};
use rara_observability::InferenceTask;
use tokio::sync::{broadcast, watch};

use super::host_driver::{HostControl, HostDriver, HostEvents, HostState};
use crate::{
    EventLog, EventStream, NoPendingInput, RuntimeControlEvent, RuntimeSessionError,
    RuntimeSessionId, RuntimeTurn, RuntimeTurnId, SessionHandle, SessionSnapshot, TurnStopKind,
};

pub type RuntimeSessionSnapshot = SessionSnapshot<NoPendingInput>;
pub type RuntimeEventStream = EventStream<RuntimeControlEvent, NoPendingInput>;

pub struct RuntimeSessionSubscription {
    pub snapshot: RuntimeSessionSnapshot,
    pub events: RuntimeEventStream,
}

/// Host-owned assembly: no ambient providers, extensions, memory, or file stores.
pub struct RuntimeSessionBuilder {
    backend: Arc<dyn LlmBackend>,
    tools: ToolManager,
    workspace_root: PathBuf,
    session_id: String,
    transcript: Vec<Message>,
    system_prompt: Option<String>,
    max_turns: Option<usize>,
    command_capacity: usize,
    event_capacity: usize,
}

impl RuntimeSessionBuilder {
    /// Inject all executable authority. Tools own permission checks and any
    /// asynchronous user interaction required before returning their results.
    pub fn for_host(
        workspace_root: impl AsRef<Path>,
        backend: Arc<dyn LlmBackend>,
        tools: ToolManager,
    ) -> Self {
        Self {
            backend,
            tools,
            workspace_root: workspace_root.as_ref().to_path_buf(),
            session_id: uuid::Uuid::new_v4().to_string(),
            transcript: Vec::new(),
            system_prompt: None,
            max_turns: None,
            command_capacity: 32,
            event_capacity: 256,
        }
    }

    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = id.into();
        self
    }
    pub fn with_transcript(mut self, transcript: Vec<Message>) -> Self {
        self.transcript = transcript;
        self
    }
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
    pub fn with_max_turns(mut self, limit: usize) -> Self {
        self.max_turns = Some(limit);
        self
    }
    pub fn with_command_capacity(mut self, capacity: usize) -> Self {
        self.command_capacity = capacity;
        self
    }
    pub fn with_event_capacity(mut self, capacity: usize) -> Self {
        self.event_capacity = capacity;
        self
    }

    pub async fn build(self) -> Result<RuntimeSession> {
        if self.session_id.trim().is_empty() {
            bail!("host session identity must not be empty");
        }
        if !self.workspace_root.is_absolute() {
            bail!("host workspace root must be absolute");
        }
        let id = RuntimeSessionId::new(self.session_id);
        let events = HostEvents {
            session_id: id.clone(),
            log: Arc::new(EventLog::new(self.event_capacity)),
        };
        let workspace_root = Arc::new(self.workspace_root);
        let driver = HostDriver {
            state: Some(HostState {
                backend: self.backend,
                tools: self.tools,
                transcript: self.transcript,
                workspace_root: workspace_root.as_ref().clone(),
                system_prompt: self.system_prompt,
                max_turns: self.max_turns,
            }),
            events: events.clone(),
        };
        Ok(RuntimeSession {
            inner: SessionHandle::start(id, driver, self.command_capacity),
            workspace_root,
            events,
        })
    }
}

/// A host-controlled session using the same actor as the native application.
#[derive(Clone)]
pub struct RuntimeSession {
    inner: SessionHandle<HostDriver>,
    workspace_root: Arc<PathBuf>,
    events: HostEvents,
}

impl RuntimeSession {
    pub fn id(&self) -> &RuntimeSessionId {
        self.inner.id()
    }
    pub fn workspace_root(&self) -> &Path {
        self.workspace_root.as_path()
    }
    pub fn snapshot(&self) -> RuntimeSessionSnapshot {
        self.inner.snapshot()
    }
    pub fn subscribe_snapshots(&self) -> watch::Receiver<RuntimeSessionSnapshot> {
        self.inner.subscribe_snapshots()
    }
    pub fn subscribe_control(&self) -> broadcast::Receiver<RuntimeControlEvent> {
        self.events.log.subscribe()
    }

    pub fn subscribe_from_snapshot(
        &self,
    ) -> Result<RuntimeSessionSubscription, RuntimeSessionError> {
        let live = self.subscribe_control();
        let snapshot = self.snapshot();
        let events = self.event_stream(live, snapshot.last_sequence)?;
        Ok(RuntimeSessionSubscription { snapshot, events })
    }

    pub fn replay_events(
        &self,
        after_sequence: u64,
    ) -> Result<Vec<RuntimeControlEvent>, RuntimeSessionError> {
        self.events
            .log
            .replay_after(after_sequence)
            .map_err(Into::into)
    }

    pub fn subscribe_after(
        &self,
        after_sequence: u64,
    ) -> Result<RuntimeEventStream, RuntimeSessionError> {
        self.event_stream(self.subscribe_control(), after_sequence)
    }

    fn event_stream(
        &self,
        live: broadcast::Receiver<RuntimeControlEvent>,
        after_sequence: u64,
    ) -> Result<RuntimeEventStream, RuntimeSessionError> {
        let replay = self.replay_events(after_sequence)?;
        Ok(EventStream::new(
            self.events.log.clone(),
            live,
            self.subscribe_snapshots(),
            replay,
            after_sequence,
        ))
    }

    pub async fn submit(
        &self,
        prompt: impl Into<String>,
    ) -> Result<RuntimeTurn, RuntimeSessionError> {
        self.submit_with_accounting(prompt, InferenceTask::default())
            .await
    }
    pub async fn submit_with_accounting(
        &self,
        prompt: impl Into<String>,
        accounting: InferenceTask,
    ) -> Result<RuntimeTurn, RuntimeSessionError> {
        self.inner.submit(prompt.into(), accounting).await
    }
    pub async fn cancel(&self) -> Result<RuntimeTurnId, RuntimeSessionError> {
        self.inner.stop_turn(None, TurnStopKind::Cancel).await
    }
    pub async fn cancel_turn(
        &self,
        turn_id: &RuntimeTurnId,
    ) -> Result<RuntimeTurnId, RuntimeSessionError> {
        self.inner
            .stop_turn(Some(turn_id.clone()), TurnStopKind::Cancel)
            .await
    }
    pub async fn interrupt_turn(
        &self,
        turn_id: &RuntimeTurnId,
    ) -> Result<RuntimeTurnId, RuntimeSessionError> {
        self.inner
            .stop_turn(Some(turn_id.clone()), TurnStopKind::Interrupt)
            .await
    }
    pub async fn transcript(&self) -> Result<Vec<Message>, RuntimeSessionError> {
        self.inner.transcript().await
    }
    pub async fn replace_transcript(
        &self,
        transcript: Vec<Message>,
    ) -> Result<(), RuntimeSessionError> {
        self.inner.replace_transcript(transcript).await
    }
    pub async fn query_runtime_state(&self) -> Result<(), RuntimeSessionError> {
        self.inner.query_runtime_state().await
    }
    pub async fn replace_llm_backend(
        &self,
        backend: Arc<dyn LlmBackend>,
    ) -> Result<(), RuntimeSessionError> {
        self.inner
            .control(HostControl::ReplaceBackend(backend))
            .await
    }
    pub async fn set_max_turns(&self, max_turns: usize) -> Result<(), RuntimeSessionError> {
        self.inner
            .control(HostControl::SetMaxTurns(Some(max_turns)))
            .await
    }
    /// Remove the per-turn model iteration limit.
    pub async fn clear_turn_limit(&self) -> Result<(), RuntimeSessionError> {
        self.inner.control(HostControl::SetMaxTurns(None)).await
    }
    pub async fn shutdown(&self) -> Result<(), RuntimeSessionError> {
        self.inner.shutdown().await
    }
}
