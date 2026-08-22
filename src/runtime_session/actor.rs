use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};

use anyhow::Result;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use super::subscription::replay_gap_error;
use super::{
    RuntimeEventStream, RuntimeSessionBuilder, RuntimeSessionError, RuntimeSessionId,
    RuntimeSessionPhase, RuntimeSessionSnapshot, RuntimeSessionSubscription, RuntimeTurn,
    RuntimeTurnId, RuntimeTurnOutcome,
};
use crate::agent::{Agent, AgentEvent, AgentOutputMode};
use crate::llm::{LlmBackend, Message};
use crate::memory_lifecycle::MemorySyncReason;
use crate::model_observation::QueryReport;
use crate::runtime_client::RuntimeClient;
use crate::runtime_context::RuntimeBootstrap;
use crate::runtime_control::{RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, SessionEvent};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tools::agent::{AgentTreeConfig, AgentTreeControl};

type TurnResultSender = oneshot::Sender<Result<RuntimeTurnOutcome, RuntimeSessionError>>;

enum SessionCommand {
    StartTurn {
        turn_id: RuntimeTurnId,
        prompt: String,
        output_mode: AgentOutputMode,
        accepted: oneshot::Sender<Result<(), RuntimeSessionError>>,
        completed: TurnResultSender,
    },
    Cancel {
        response: oneshot::Sender<Result<RuntimeTurnId, RuntimeSessionError>>,
    },
    ReplaceBackend {
        backend: Arc<dyn LlmBackend>,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    SetMaxTurns {
        max_turns: usize,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    DisableTools {
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    DisableExtensionExecution {
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    SetFullAccess {
        enabled: bool,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    GetTranscript {
        response: oneshot::Sender<Result<Vec<Message>, RuntimeSessionError>>,
    },
    ReplaceTranscript {
        transcript: Vec<Message>,
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
    Shutdown {
        response: oneshot::Sender<Result<(), RuntimeSessionError>>,
    },
}

struct ActiveTurn {
    turn_id: RuntimeTurnId,
    generation: u64,
    cancellation: Arc<AtomicBool>,
    completed: TurnResultSender,
}

struct TurnCompletion {
    turn_id: RuntimeTurnId,
    generation: u64,
    agent: Agent,
    result: Result<()>,
    query_report: QueryReport,
}

/// Cloneable command and observation handle for one runtime session.
#[derive(Clone)]
pub struct RuntimeSession {
    id: RuntimeSessionId,
    workspace_root: Arc<PathBuf>,
    commands: mpsc::Sender<SessionCommand>,
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
        let session = Self {
            id: id.clone(),
            workspace_root: Arc::new(workspace_root),
            commands,
            snapshot: snapshot_receiver,
            event_bus: event_bus.clone(),
            agent_tree_control,
        };
        tokio::spawn(async move {
            SessionActor::new(
                id,
                client,
                command_receiver,
                snapshot_sender,
                actor_agent_tree_control,
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
        let replay = self
            .event_bus
            .replay_after(snapshot.last_sequence)
            .map_err(replay_gap_error)?;
        let events = RuntimeEventStream::new(
            self.event_bus.clone(),
            live,
            self.snapshot.clone(),
            replay,
            snapshot.last_sequence,
        );
        Ok(RuntimeSessionSubscription { snapshot, events })
    }

    /// Submit one prompt. A busy session rejects rather than running two root turns.
    pub async fn submit(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
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
        })?;
        accepted_receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)??;
        Ok(RuntimeTurn::new(turn_id, completion_receiver))
    }

    /// Execute a prompt and stream its typed events to the caller.
    pub async fn query_with_events<F>(
        &self,
        prompt: impl Into<String>,
        output_mode: AgentOutputMode,
        mut report: F,
    ) -> Result<RuntimeTurnOutcome, RuntimeSessionError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut events = self.subscribe_events();
        let turn = self.submit(prompt, output_mode).await?;
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
            return Ok(());
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
                return Ok(());
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
}

struct SessionActor {
    id: RuntimeSessionId,
    client: RuntimeClient,
    commands: mpsc::Receiver<SessionCommand>,
    completions: mpsc::Sender<TurnCompletion>,
    completion_receiver: mpsc::Receiver<TurnCompletion>,
    snapshot: watch::Sender<RuntimeSessionSnapshot>,
    agent_tree_control: Arc<AgentTreeControl>,
    generation: u64,
    active: Option<ActiveTurn>,
    closing: bool,
    shutdown_waiters: Vec<oneshot::Sender<Result<(), RuntimeSessionError>>>,
}

impl SessionActor {
    fn new(
        id: RuntimeSessionId,
        client: RuntimeClient,
        commands: mpsc::Receiver<SessionCommand>,
        snapshot: watch::Sender<RuntimeSessionSnapshot>,
        agent_tree_control: Arc<AgentTreeControl>,
    ) -> Self {
        let (completions, completion_receiver) = mpsc::channel(1);
        Self {
            id,
            client,
            commands,
            completions,
            completion_receiver,
            snapshot,
            agent_tree_control,
            generation: 0,
            active: None,
            closing: false,
            shutdown_waiters: Vec::new(),
        }
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                completion = self.completion_receiver.recv(), if self.active.is_some() => {
                    if let Some(completion) = completion {
                        self.finish_turn(completion).await;
                    }
                    if self.closing && self.active.is_none() {
                        self.finish_shutdown().await;
                        return;
                    }
                }
                command = self.commands.recv(), if !self.closing => {
                    match command {
                        Some(command) => {
                            if self.handle_command(command).await {
                                return;
                            }
                        }
                        None => {
                            self.closing = true;
                            self.begin_agent_tree_shutdown();
                            self.cancel_active();
                            self.publish_snapshot(RuntimeSessionPhase::Closing);
                            if self.active.is_none() {
                                self.finish_shutdown().await;
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn handle_command(&mut self, command: SessionCommand) -> bool {
        if self.closing && !matches!(command, SessionCommand::Shutdown { .. }) {
            Self::reject_closed(command);
            return false;
        }

        match command {
            SessionCommand::StartTurn {
                turn_id,
                prompt,
                output_mode,
                accepted,
                completed,
            } => {
                if let Some(active) = &self.active {
                    let _ = accepted.send(Err(RuntimeSessionError::Busy {
                        active_turn: active.turn_id.clone(),
                    }));
                    return false;
                }
                let Some(mut agent) = self.client.agent_mut().take() else {
                    let _ = accepted.send(Err(RuntimeSessionError::ActorStopped));
                    return false;
                };
                let cancellation = Arc::new(AtomicBool::new(false));
                agent.set_cancellation_token(Some(cancellation.clone()));
                agent.set_runtime_turn_id(Some(turn_id.to_string()));
                let active = ActiveTurn {
                    turn_id: turn_id.clone(),
                    generation: self.generation,
                    cancellation,
                    completed,
                };
                self.active = Some(active);
                self.publish_agent_event(&turn_id, AgentEvent::AgentStart);
                self.publish_snapshot(RuntimeSessionPhase::Running {
                    turn_id: turn_id.clone(),
                });

                let completions = self.completions.clone();
                let event_bus = self.client.event_bus.clone();
                let provenance = RuntimeProvenance::runtime(Some(self.id.to_string()));
                let generation = self.generation;
                let execution_turn_id = turn_id.clone();
                tokio::spawn(async move {
                    let event_turn_id = execution_turn_id.clone();
                    let result = agent
                        .query_with_mode_and_events(prompt, output_mode, move |event| {
                            event_bus.send_with_turn(
                                event,
                                provenance.clone(),
                                Some(event_turn_id.as_str()),
                            );
                        })
                        .await;
                    let query_report = agent.last_query_report.clone();
                    let completion = TurnCompletion {
                        turn_id: execution_turn_id,
                        generation,
                        agent,
                        result,
                        query_report,
                    };
                    if completions.send(completion).await.is_err() {
                        log::warn!(
                            "runtime session actor stopped before accepting turn completion"
                        );
                    }
                });
                let _ = accepted.send(Ok(()));
            }
            SessionCommand::Cancel { response } => {
                let result = if let Some(active) = &self.active {
                    let turn_id = active.turn_id.clone();
                    self.cancel_active();
                    self.publish_snapshot(RuntimeSessionPhase::Cancelling {
                        turn_id: turn_id.clone(),
                    });
                    Ok(turn_id)
                } else {
                    Err(RuntimeSessionError::NotRunning)
                };
                let _ = response.send(result);
            }
            SessionCommand::ReplaceBackend { backend, response } => {
                let result = self.with_idle_agent(|agent| agent.llm_backend = backend);
                let _ = response.send(result);
            }
            SessionCommand::SetMaxTurns {
                max_turns,
                response,
            } => {
                let result = self.with_idle_agent(|agent| agent.set_max_turns(max_turns));
                let _ = response.send(result);
            }
            SessionCommand::DisableTools { response } => {
                let result = self.with_idle_agent(|agent| agent.tool_manager.retain(|_| false));
                let _ = response.send(result);
            }
            SessionCommand::DisableExtensionExecution { response } => {
                let result = self.with_idle_agent(Agent::disable_extension_execution);
                let _ = response.send(result);
            }
            SessionCommand::SetFullAccess { enabled, response } => {
                let result = self.with_idle_agent(|agent| agent.set_full_access_mode(enabled));
                let _ = response.send(result);
            }
            SessionCommand::GetTranscript { response } => {
                let result = match &self.active {
                    Some(active) => Err(RuntimeSessionError::Busy {
                        active_turn: active.turn_id.clone(),
                    }),
                    None => self
                        .client
                        .agent()
                        .map(|agent| agent.history.clone())
                        .ok_or(RuntimeSessionError::ActorStopped),
                };
                let _ = response.send(result);
            }
            SessionCommand::ReplaceTranscript {
                transcript,
                response,
            } => {
                let result = self.with_idle_agent(|agent| agent.replace_history(transcript));
                let _ = response.send(result);
            }
            SessionCommand::Shutdown { response } => {
                self.closing = true;
                self.shutdown_waiters.push(response);
                self.begin_agent_tree_shutdown();
                self.cancel_active();
                self.publish_snapshot(RuntimeSessionPhase::Closing);
                self.commands.close();
                while let Ok(command) = self.commands.try_recv() {
                    Self::reject_closed(command);
                }
                if self.active.is_none() {
                    self.finish_shutdown().await;
                    return true;
                }
            }
        }
        false
    }

    fn with_idle_agent(
        &mut self,
        update: impl FnOnce(&mut Agent),
    ) -> Result<(), RuntimeSessionError> {
        if let Some(active) = &self.active {
            return Err(RuntimeSessionError::Busy {
                active_turn: active.turn_id.clone(),
            });
        }
        let agent = self
            .client
            .agent_mut()
            .as_mut()
            .ok_or(RuntimeSessionError::ActorStopped)?;
        update(agent);
        Ok(())
    }

    async fn finish_turn(&mut self, mut completion: TurnCompletion) {
        let Some(active) = self.active.take() else {
            log::warn!("runtime session received a completion without an active turn");
            return;
        };
        if completion.generation != active.generation
            || completion.turn_id != active.turn_id
            || completion.generation != self.generation
        {
            log::warn!(
                "runtime session rejected stale turn completion for {}",
                completion.turn_id
            );
            let _ = active
                .completed
                .send(Err(RuntimeSessionError::ActorStopped));
            return;
        }

        completion.agent.set_cancellation_token(None);
        completion.agent.set_runtime_turn_id(None);
        let cancelled = active.cancellation.load(Ordering::SeqCst);
        *self.client.agent_mut() = Some(completion.agent);
        if let Some(agent) = self.client.agent() {
            self.client
                .capture_memory(agent, MemorySyncReason::TurnIdle)
                .await;
        }

        let outcome = RuntimeTurnOutcome {
            turn_id: completion.turn_id.clone(),
            query_report: completion.query_report,
            transcript: self
                .client
                .agent()
                .map(|agent| agent.history.clone())
                .unwrap_or_default(),
        };

        let result = if cancelled {
            self.publish_terminal_event(
                &completion.turn_id,
                "cancelled",
                SessionEvent::TurnCancelled,
            );
            Err(RuntimeSessionError::Cancelled { outcome })
        } else {
            match completion.result {
                Ok(()) => {
                    self.publish_terminal_event(
                        &completion.turn_id,
                        "completed",
                        SessionEvent::TurnFinished {
                            reason: Some("completed".to_string()),
                        },
                    );
                    Ok(outcome)
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    self.publish_agent_event(
                        &completion.turn_id,
                        AgentEvent::AgentError {
                            message: message.clone(),
                            recoverable: false,
                        },
                    );
                    self.publish_terminal_event(
                        &completion.turn_id,
                        "failed",
                        SessionEvent::TurnFailed {
                            reason: message.clone(),
                        },
                    );
                    Err(RuntimeSessionError::Execution { message, outcome })
                }
            }
        };
        if !self.closing {
            self.publish_snapshot(RuntimeSessionPhase::Idle);
        }
        let _ = active.completed.send(result);
    }

    fn cancel_active(&self) {
        if let Some(active) = &self.active {
            active.cancellation.store(true, Ordering::SeqCst);
        }
        if let Err(error) = self.agent_tree_control.cancel_running() {
            log::warn!("failed to cancel active sub-agents: {error}");
        }
    }

    fn begin_agent_tree_shutdown(&self) {
        if let Err(error) = self.agent_tree_control.begin_shutdown() {
            log::warn!("failed to close sub-agent admission: {error}");
        }
    }

    async fn finish_shutdown(&mut self) {
        if let Err(error) = self.agent_tree_control.shutdown().await {
            log::warn!("failed to shut down session sub-agents: {error}");
        }
        self.client.drain_memory().await;
        self.publish_snapshot(RuntimeSessionPhase::Closed);
        for waiter in self.shutdown_waiters.drain(..) {
            let _ = waiter.send(Ok(()));
        }
    }

    fn publish_agent_event(&self, turn_id: &RuntimeTurnId, event: AgentEvent) {
        self.client.event_bus.send_with_turn(
            event,
            RuntimeProvenance::runtime(Some(self.id.to_string())),
            Some(turn_id.as_str()),
        );
    }

    fn publish_terminal_event(&self, turn_id: &RuntimeTurnId, reason: &str, event: SessionEvent) {
        self.client.event_bus.publish_raw(AgentEvent::AgentStop {
            reason: reason.to_string(),
        });
        self.client.event_bus.publish_control_with_turn(
            RuntimeEvent::Session(event),
            RuntimeProvenance::runtime(Some(self.id.to_string())),
            Some(turn_id.as_str()),
        );
    }

    fn publish_snapshot(&self, phase: RuntimeSessionPhase) {
        self.snapshot.send_replace(RuntimeSessionSnapshot {
            session_id: self.id.clone(),
            phase,
            generation: self.generation,
            last_sequence: self.client.event_bus.current_sequence(),
        });
    }

    fn reject_closed(command: SessionCommand) {
        match command {
            SessionCommand::StartTurn { accepted, .. } => {
                let _ = accepted.send(Err(RuntimeSessionError::Closed));
            }
            SessionCommand::Cancel { response } => {
                let _ = response.send(Err(RuntimeSessionError::Closed));
            }
            SessionCommand::ReplaceBackend { response, .. }
            | SessionCommand::SetMaxTurns { response, .. }
            | SessionCommand::DisableTools { response }
            | SessionCommand::DisableExtensionExecution { response }
            | SessionCommand::SetFullAccess { response, .. }
            | SessionCommand::ReplaceTranscript { response, .. } => {
                let _ = response.send(Err(RuntimeSessionError::Closed));
            }
            SessionCommand::GetTranscript { response } => {
                let _ = response.send(Err(RuntimeSessionError::Closed));
            }
            SessionCommand::Shutdown { response } => {
                let _ = response.send(Err(RuntimeSessionError::Closed));
            }
        }
    }
}
