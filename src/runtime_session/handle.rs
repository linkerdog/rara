use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use super::actor::SessionActor;
use super::command::SessionCommand;
use super::shutdown::ShutdownOutcome;
use super::subscription::replay_gap_error;
use super::{
    RuntimeEventStream, RuntimeSessionBuilder, RuntimeSessionError, RuntimeSessionId,
    RuntimeSessionPhase, RuntimeSessionSnapshot, RuntimeSessionSubscription, RuntimeTurn,
    RuntimeTurnId, RuntimeTurnOutcome,
};
use crate::agent::{AgentEvent, AgentOutputMode};
use crate::llm::{LlmBackend, Message};
use crate::model_observation::QueryReport;
use crate::runtime_client::RuntimeClient;
use crate::runtime_context::RuntimeBootstrap;
use crate::runtime_control::RuntimeControlEvent;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tools::agent::{AgentTreeConfig, AgentTreeControl};

/// Cloneable command and observation handle for one runtime session.
#[derive(Clone)]
pub struct RuntimeSession {
    id: RuntimeSessionId,
    workspace_root: Arc<PathBuf>,
    commands: mpsc::Sender<SessionCommand>,
    snapshot: watch::Receiver<RuntimeSessionSnapshot>,
    event_bus: Arc<RuntimeEventBus>,
    agent_tree_control: Arc<AgentTreeControl>,
    shutdown_outcome: Arc<OnceLock<ShutdownOutcome>>,
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
        let actor_agent_tree_control = agent_tree_control.clone();
        let event_bus = client.event_bus.clone();
        let snapshot = RuntimeSessionSnapshot {
            session_id: id.clone(),
            phase: RuntimeSessionPhase::Idle,
            generation: 0,
            last_sequence: event_bus.current_sequence(),
        };
        let (snapshot_sender, snapshot_receiver) = watch::channel(snapshot);
        let (commands, command_receiver) = mpsc::channel(command_capacity.max(1));
        let shutdown_outcome = Arc::new(OnceLock::new());
        let session = Self {
            id: id.clone(),
            workspace_root: Arc::new(workspace_root),
            commands,
            snapshot: snapshot_receiver,
            event_bus: event_bus.clone(),
            agent_tree_control,
            shutdown_outcome: shutdown_outcome.clone(),
        };
        tokio::spawn(async move {
            SessionActor::new(
                id,
                client,
                command_receiver,
                snapshot_sender,
                actor_agent_tree_control,
                shutdown_outcome,
            )
            .run()
            .await;
        });
        Ok(session)
    }

    /// Return the stable session identity.
    pub fn id(&self) -> &RuntimeSessionId {
        &self.id
    }

    pub(super) fn same_actor(&self, other: &Self) -> bool {
        self.commands.same_channel(&other.commands)
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
            self.event_bus.clone(),
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
        let turn_id = RuntimeTurnId::generate();
        let (accepted_sender, accepted_receiver) = oneshot::channel();
        let (completion_sender, completion_receiver) = oneshot::channel();
        self.try_send(SessionCommand::StartTurn {
            turn_id: turn_id.clone(),
            prompt: prompt.into(),
            output_mode,
            accepted: accepted_sender,
            completed: completion_sender,
            inference_agent: accounting.start_agent(None),
        })?;
        accepted_receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)??;
        Ok(RuntimeTurn::new(turn_id, completion_receiver, accounting))
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
        let (sender, receiver) = oneshot::channel();
        self.try_send(SessionCommand::Cancel { response: sender })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    /// Return a consistent transcript snapshot while the session is idle.
    pub async fn transcript(&self) -> Result<Vec<Message>, RuntimeSessionError> {
        let (sender, receiver) = oneshot::channel();
        self.try_send(SessionCommand::GetTranscript { response: sender })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    /// Replace the transcript while idle, for host-controlled hydration.
    pub async fn replace_transcript(
        &self,
        transcript: Vec<Message>,
    ) -> Result<(), RuntimeSessionError> {
        let (sender, receiver) = oneshot::channel();
        self.try_send(SessionCommand::ReplaceTranscript {
            transcript,
            response: sender,
        })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    /// Drain the session-owned memory lifecycle and stop the actor.
    pub async fn shutdown(&self) -> Result<(), RuntimeSessionError> {
        if matches!(self.snapshot().phase, RuntimeSessionPhase::Closed) {
            return self.shutdown_result();
        }
        if matches!(self.snapshot().phase, RuntimeSessionPhase::Closing) {
            return self.wait_until_closed().await;
        }
        let (sender, receiver) = oneshot::channel();
        if self
            .commands
            .send(SessionCommand::Shutdown { response: sender })
            .await
            .is_err()
        {
            return if self.is_closing_or_closed() {
                self.wait_until_closed().await
            } else {
                Err(RuntimeSessionError::ActorStopped)
            };
        }
        match receiver.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(RuntimeSessionError::Closed)) if self.is_closing_or_closed() => {
                self.wait_until_closed().await
            }
            Ok(Err(error)) => Err(error),
            Err(_) if self.is_closing_or_closed() => self.wait_until_closed().await,
            Err(_) => Err(RuntimeSessionError::ActorStopped),
        }
    }

    /// Replace the provider backend while the session is idle.
    pub async fn replace_llm_backend(
        &self,
        backend: Arc<dyn LlmBackend>,
    ) -> Result<(), RuntimeSessionError> {
        let (sender, receiver) = oneshot::channel();
        self.try_send(SessionCommand::ReplaceBackend {
            backend,
            response: sender,
        })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    /// Set the maximum number of model turns allowed for each submitted turn.
    pub async fn set_max_turns(&self, max_turns: usize) -> Result<(), RuntimeSessionError> {
        let (sender, receiver) = oneshot::channel();
        self.try_send(SessionCommand::SetMaxTurns {
            max_turns,
            response: sender,
        })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    pub(crate) async fn disable_tools(&self) -> Result<(), RuntimeSessionError> {
        let (sender, receiver) = oneshot::channel();
        self.try_send(SessionCommand::DisableTools { response: sender })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    pub(crate) async fn disable_extension_execution(&self) -> Result<(), RuntimeSessionError> {
        let (sender, receiver) = oneshot::channel();
        self.try_send(SessionCommand::DisableExtensionExecution { response: sender })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    /// Change the local tool-approval policy while the session is idle.
    pub async fn set_full_access_mode(&self, enabled: bool) -> Result<(), RuntimeSessionError> {
        let (sender, receiver) = oneshot::channel();
        self.try_send(SessionCommand::SetFullAccess {
            enabled,
            response: sender,
        })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    fn try_send(&self, command: SessionCommand) -> Result<(), RuntimeSessionError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RuntimeSessionError::Overloaded,
                mpsc::error::TrySendError::Closed(_) => RuntimeSessionError::Closed,
            })
    }

    async fn wait_until_closed(&self) -> Result<(), RuntimeSessionError> {
        let mut snapshot = self.snapshot.clone();
        loop {
            if matches!(snapshot.borrow().phase, RuntimeSessionPhase::Closed) {
                return self.shutdown_result();
            }
            snapshot
                .changed()
                .await
                .map_err(|_| RuntimeSessionError::ActorStopped)?;
        }
    }

    fn is_closing_or_closed(&self) -> bool {
        matches!(
            self.snapshot().phase,
            RuntimeSessionPhase::Closing | RuntimeSessionPhase::Closed
        )
    }

    fn shutdown_result(&self) -> Result<(), RuntimeSessionError> {
        self.shutdown_outcome
            .get()
            .ok_or(RuntimeSessionError::ActorStopped)?
            .result()
    }
}
