use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use rara_runtime::{SessionHandle, TurnStopKind};
use tokio::sync::{broadcast, watch};

use super::command::NativeControl;
use super::driver::{NativeInput, NativeSessionDriver};
use super::input::TurnInput;
use super::subscription::replay_gap_error;
use super::{
    RuntimeEventStream, RuntimeInput, RuntimeSessionBuilder, RuntimeSessionError, RuntimeSessionId,
    RuntimeSessionSnapshot, RuntimeSessionSubscription, RuntimeTurn, RuntimeTurnId,
    RuntimeTurnOutcome,
};
use crate::agent::{AgentEvent, AgentOutputMode};
use crate::llm::{LlmBackend, Message};
use crate::model_observation::QueryReport;
use crate::runtime_client::RuntimeClient;
use crate::runtime_context::RuntimeBootstrap;
use crate::runtime_control::{
    PromptSourceControlRequest, RuntimeControlEvent, RuntimeProvenance, SkillSourceControlRequest,
};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tools::agent::{AgentTreeConfig, AgentTreeControl};

/// Cloneable command and observation handle for one runtime session.
#[derive(Clone)]
pub struct RuntimeSession {
    id: RuntimeSessionId,
    workspace_root: Arc<PathBuf>,
    inner: SessionHandle<NativeSessionDriver>,
    snapshot: watch::Receiver<RuntimeSessionSnapshot>,
    event_bus: Arc<RuntimeEventBus>,
    agent_tree_control: Arc<AgentTreeControl>,
}

impl RuntimeSession {
    /// Start building one session from application configuration.
    pub fn builder(
        config: crate::RaraConfig,
        workspace_root: impl AsRef<Path>,
    ) -> RuntimeSessionBuilder {
        RuntimeSessionBuilder::new(config, workspace_root)
    }

    pub(crate) async fn from_bootstrap(bootstrap: RuntimeBootstrap) -> Result<Self> {
        let client = RuntimeClient::from_bootstrap(bootstrap).await;
        Self::start(client, super::builder::DEFAULT_COMMAND_CAPACITY)
    }

    pub(crate) fn start(client: RuntimeClient, command_capacity: usize) -> Result<Self> {
        let agent = client
            .agent()
            .ok_or_else(|| anyhow::anyhow!("runtime bootstrap did not produce an agent"))?;
        let id = RuntimeSessionId::new(agent.session_id.clone());
        let workspace_root = agent.workspace.root.clone();
        let agent_tree_control = agent
            .agent_tree_control()
            .unwrap_or_else(|| Arc::new(AgentTreeControl::new(AgentTreeConfig::default())));
        let event_bus = client.event_bus.clone();
        let driver = NativeSessionDriver::new(id.clone(), client, agent_tree_control.clone());
        let inner = SessionHandle::start(id.clone(), driver, command_capacity);
        let snapshot = inner.subscribe_snapshots();
        Ok(Self {
            id,
            workspace_root: Arc::new(workspace_root),
            inner,
            snapshot,
            event_bus,
            agent_tree_control,
        })
    }

    /// Return the stable session identity.
    pub fn id(&self) -> &RuntimeSessionId {
        &self.id
    }

    pub(super) fn same_actor(&self, other: &Self) -> bool {
        self.inner.same_actor(&other.inner)
    }

    /// Return the workspace owned by this session.
    pub fn workspace_root(&self) -> &Path {
        self.workspace_root.as_path()
    }

    /// Clone the session-scoped child-agent control handle.
    pub fn agent_tree_control(&self) -> Arc<AgentTreeControl> {
        self.agent_tree_control.clone()
    }

    /// Read the latest lifecycle snapshot without waiting for the actor.
    pub fn snapshot(&self) -> RuntimeSessionSnapshot {
        self.snapshot.borrow().clone()
    }

    /// Subscribe to coalesced lifecycle snapshots for this session.
    pub fn subscribe_snapshots(&self) -> watch::Receiver<RuntimeSessionSnapshot> {
        self.snapshot.clone()
    }

    /// Subscribe to raw typed agent events for compatibility consumers.
    pub fn subscribe_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_bus.subscribe()
    }

    /// Subscribe to ordered protocol events.
    pub fn subscribe_control(&self) -> broadcast::Receiver<RuntimeControlEvent> {
        self.event_bus.subscribe_control()
    }

    /// Atomically pair the latest snapshot with replay and a live event stream.
    pub fn subscribe_from_snapshot(
        &self,
    ) -> Result<RuntimeSessionSubscription, RuntimeSessionError> {
        let live = self.event_bus.subscribe_control();
        let snapshot = self.snapshot();
        let events = self.event_stream(live, snapshot.last_sequence)?;
        Ok(RuntimeSessionSubscription { snapshot, events })
    }

    /// Read a finite bounded replay without allocating replacement event identities.
    pub fn replay_events(
        &self,
        after_sequence: u64,
    ) -> Result<Vec<RuntimeControlEvent>, RuntimeSessionError> {
        self.event_bus
            .replay_after(after_sequence)
            .map_err(replay_gap_error)
    }

    /// Publish session state through the canonical ordered control stream.
    pub async fn query_runtime_state(&self) -> Result<(), RuntimeSessionError> {
        self.inner.query_runtime_state().await
    }

    /// Replay after an exclusive cursor, then follow the same ordered live stream.
    pub fn subscribe_after(
        &self,
        after_sequence: u64,
    ) -> Result<RuntimeEventStream, RuntimeSessionError> {
        self.event_stream(self.event_bus.subscribe_control(), after_sequence)
    }

    fn event_stream(
        &self,
        live: broadcast::Receiver<RuntimeControlEvent>,
        after_sequence: u64,
    ) -> Result<RuntimeEventStream, RuntimeSessionError> {
        let replay = self
            .event_bus
            .replay_after(after_sequence)
            .map_err(replay_gap_error)?;
        Ok(RuntimeEventStream::new(
            self.event_bus.control_log(),
            live,
            self.snapshot.clone(),
            replay,
            after_sequence,
        ))
    }

    /// Submit one prompt. A busy session rejects rather than running two root turns.
    pub async fn submit(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
    ) -> Result<RuntimeTurn, RuntimeSessionError> {
        self.submit_with_accounting(
            prompt,
            output_mode,
            rara_observability::InferenceTask::default(),
        )
        .await
    }

    /// Submit a prompt using an explicit task ledger that also follows descendants.
    pub async fn submit_with_accounting(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        accounting: rara_observability::InferenceTask,
    ) -> Result<RuntimeTurn, RuntimeSessionError> {
        self.submit_turn(
            TurnInput::LegacyPrompt(prompt.into()),
            output_mode,
            accounting,
        )
        .await
    }

    /// Submit strict protocol input without bypassing a pending question or approval.
    pub async fn submit_input(
        &self,
        input: RuntimeInput,
    ) -> Result<RuntimeTurn, RuntimeSessionError> {
        self.submit_input_with_accounting(input, rara_observability::InferenceTask::default())
            .await
    }

    /// Submit strict input with an explicit ledger inherited by its native descendants.
    pub async fn submit_input_with_accounting(
        &self,
        input: RuntimeInput,
        accounting: rara_observability::InferenceTask,
    ) -> Result<RuntimeTurn, RuntimeSessionError> {
        self.submit_turn(
            TurnInput::Controlled(input),
            AgentOutputMode::Silent,
            accounting,
        )
        .await
    }

    async fn submit_turn(
        &self,
        input: TurnInput,
        output_mode: AgentOutputMode,
        accounting: rara_observability::InferenceTask,
    ) -> Result<RuntimeTurn, RuntimeSessionError> {
        self.inner
            .submit(NativeInput { input, output_mode }, accounting)
            .await
    }

    /// Execute a prompt and stream its typed events to the caller.
    pub async fn query_with_events<F>(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        report: F,
    ) -> Result<RuntimeTurnOutcome, RuntimeSessionError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.query_with_accounting(
            prompt,
            output_mode,
            rara_observability::InferenceTask::default(),
            report,
        )
        .await
    }

    /// Retain the supplied handle to inspect costs on error or after late children finish.
    pub async fn query_with_accounting<F>(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        accounting: rara_observability::InferenceTask,
        mut report: F,
    ) -> Result<RuntimeTurnOutcome, RuntimeSessionError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut events = self.subscribe_events();
        let turn = self
            .submit_with_accounting(prompt, output_mode, accounting)
            .await?;
        let mut completion = Box::pin(turn.wait());
        let mut outcome = None;
        let mut terminal_seen = false;

        loop {
            tokio::select! {
                result = &mut completion, if outcome.is_none() => {
                    if matches!(&result, Err(RuntimeSessionError::ActorStopped)) {
                        return result;
                    }
                    outcome = Some(result);
                }
                event = events.recv(), if !terminal_seen => {
                    match event {
                        Ok(event) => {
                            terminal_seen = matches!(event, AgentEvent::AgentStop { .. });
                            report(event);
                        }
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            return Err(RuntimeSessionError::EventLagged(count));
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(RuntimeSessionError::ActorStopped);
                        }
                    }
                }
            }

            if terminal_seen && let Some(outcome) = outcome {
                return outcome;
            }
        }
    }

    /// Execute a prompt and return its structured model observations.
    pub async fn query_with_report<F>(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        report: F,
    ) -> Result<QueryReport, RuntimeSessionError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        Ok(self
            .query_with_events(prompt, output_mode, report)
            .await?
            .query_report)
    }

    /// Request cancellation without waiting for the running agent to return.
    pub async fn cancel(&self) -> Result<RuntimeTurnId, RuntimeSessionError> {
        self.stop_turn(None, TurnStopKind::Cancel).await
    }

    /// Request cancellation only if the expected turn is still active.
    pub async fn cancel_turn(
        &self,
        expected_turn: &RuntimeTurnId,
    ) -> Result<RuntimeTurnId, RuntimeSessionError> {
        self.stop_turn(Some(expected_turn.clone()), TurnStopKind::Cancel)
            .await
    }

    /// Interrupt one active turn, retaining a distinct terminal interruption outcome.
    pub async fn interrupt_turn(
        &self,
        expected_turn: &RuntimeTurnId,
    ) -> Result<RuntimeTurnId, RuntimeSessionError> {
        self.stop_turn(Some(expected_turn.clone()), TurnStopKind::Interrupt)
            .await
    }

    async fn stop_turn(
        &self,
        expected_turn: Option<RuntimeTurnId>,
        kind: TurnStopKind,
    ) -> Result<RuntimeTurnId, RuntimeSessionError> {
        self.inner.stop_turn(expected_turn, kind).await
    }

    /// Return a consistent transcript snapshot while the session is idle.
    pub async fn transcript(&self) -> Result<Vec<Message>, RuntimeSessionError> {
        self.inner.transcript().await
    }

    /// Replace the transcript while idle, for host-controlled hydration.
    pub async fn replace_transcript(
        &self,
        transcript: Vec<Message>,
    ) -> Result<(), RuntimeSessionError> {
        self.inner.replace_transcript(transcript).await
    }

    /// Apply a bounded prompt source while idle, keeping its authority session-scoped.
    pub async fn apply_prompt_source(
        &self,
        request: PromptSourceControlRequest,
        mut provenance: RuntimeProvenance,
    ) -> Result<(), RuntimeSessionError> {
        if provenance
            .session_id
            .as_deref()
            .is_some_and(|id| id != self.id.as_str())
        {
            return Err(RuntimeSessionError::InvalidSource);
        }
        provenance.session_id = Some(self.id.to_string());
        self.inner
            .control(NativeControl::PromptSource {
                request,
                provenance,
            })
            .await
    }

    /// Apply bounded inline skills to the session-owned native tool catalogue.
    pub async fn apply_skill_source(
        &self,
        request: SkillSourceControlRequest,
        mut provenance: RuntimeProvenance,
    ) -> Result<(), RuntimeSessionError> {
        if provenance
            .session_id
            .as_deref()
            .is_some_and(|id| id != self.id.as_str())
        {
            return Err(RuntimeSessionError::InvalidSource);
        }
        provenance.session_id = Some(self.id.to_string());
        self.inner
            .control(NativeControl::SkillSource {
                request,
                provenance,
            })
            .await
    }

    /// Drain the session-owned memory lifecycle and stop the actor.
    pub async fn shutdown(&self) -> Result<(), RuntimeSessionError> {
        self.inner.shutdown().await
    }

    /// Replace the provider backend while the session is idle.
    pub async fn replace_llm_backend(
        &self,
        backend: Arc<dyn LlmBackend>,
    ) -> Result<(), RuntimeSessionError> {
        self.inner
            .control(NativeControl::ReplaceBackend { backend })
            .await
    }

    /// Set the maximum number of model turns allowed for each submitted turn.
    pub async fn set_max_turns(&self, max_turns: usize) -> Result<(), RuntimeSessionError> {
        self.inner
            .control(NativeControl::SetMaxTurns { max_turns })
            .await
    }

    pub(crate) async fn disable_tools(&self) -> Result<(), RuntimeSessionError> {
        self.inner.control(NativeControl::DisableTools).await
    }

    pub(crate) async fn disable_extension_execution(&self) -> Result<(), RuntimeSessionError> {
        self.inner
            .control(NativeControl::DisableExtensionExecution)
            .await
    }

    /// Change the local tool-approval policy while the session is idle.
    pub async fn set_full_access_mode(&self, enabled: bool) -> Result<(), RuntimeSessionError> {
        self.inner
            .control(NativeControl::SetFullAccess { enabled })
            .await
    }
}
