use std::sync::{Arc, OnceLock, atomic::AtomicBool, atomic::Ordering};

use anyhow::Result;
use tokio::sync::{mpsc, oneshot, watch};

use super::command::{SessionCommand, TurnResultSender, TurnStopKind};
use super::shutdown::ShutdownOutcome;
use super::{
    RuntimeSessionError, RuntimeSessionId, RuntimeSessionPhase, RuntimeSessionSnapshot,
    RuntimeTurnId, RuntimeTurnOutcome,
};
use crate::agent::{Agent, AgentEvent};
use crate::memory_lifecycle::MemorySyncReason;
use crate::model_observation::QueryReport;
use crate::runtime_client::RuntimeClient;
use crate::runtime_control::{RuntimeEvent, RuntimeProvenance, SessionEvent};
use crate::tools::agent::AgentTreeControl;

struct ActiveTurn {
    turn_id: RuntimeTurnId,
    generation: u64,
    cancellation: Arc<AtomicBool>,
    stop_kind: Option<TurnStopKind>,
    completed: TurnResultSender,
}

struct TurnCompletion {
    turn_id: RuntimeTurnId,
    generation: u64,
    agent: Agent,
    result: Result<()>,
    query_report: QueryReport,
}

pub(super) struct SessionActor {
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
    shutdown_outcome: Arc<OnceLock<ShutdownOutcome>>,
}

impl SessionActor {
    pub(super) fn new(
        id: RuntimeSessionId,
        client: RuntimeClient,
        commands: mpsc::Receiver<SessionCommand>,
        snapshot: watch::Sender<RuntimeSessionSnapshot>,
        agent_tree_control: Arc<AgentTreeControl>,
        shutdown_outcome: Arc<OnceLock<ShutdownOutcome>>,
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
            shutdown_outcome,
        }
    }

    pub(super) async fn run(mut self) {
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
                inference_agent,
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
                agent.pending_inference_agent = Some(inference_agent);
                let active = ActiveTurn {
                    turn_id: turn_id.clone(),
                    generation: self.generation,
                    cancellation,
                    stop_kind: None,
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
            SessionCommand::StopTurn {
                expected_turn,
                kind,
                response,
            } => {
                let result = self.stop_active(expected_turn, kind);
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
            match active.stop_kind.unwrap_or(TurnStopKind::Cancel) {
                TurnStopKind::Cancel => {
                    self.publish_terminal_event(
                        &completion.turn_id,
                        "cancelled",
                        SessionEvent::TurnCancelled,
                    );
                    Err(RuntimeSessionError::Cancelled { outcome })
                }
                TurnStopKind::Interrupt => {
                    self.publish_terminal_event(
                        &completion.turn_id,
                        "interrupted",
                        SessionEvent::TurnInterrupted,
                    );
                    Err(RuntimeSessionError::Interrupted { outcome })
                }
            }
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

    fn stop_active(
        &mut self,
        expected_turn: Option<RuntimeTurnId>,
        kind: TurnStopKind,
    ) -> Result<RuntimeTurnId, RuntimeSessionError> {
        let active = self
            .active
            .as_mut()
            .ok_or(RuntimeSessionError::NotRunning)?;
        if let Some(expected) = expected_turn
            && expected != active.turn_id
        {
            return Err(RuntimeSessionError::StaleTurn {
                expected,
                active: active.turn_id.clone(),
            });
        }
        if active.stop_kind.is_some_and(|accepted| accepted != kind) {
            return Err(RuntimeSessionError::StopInProgress {
                active_turn: active.turn_id.clone(),
            });
        }
        active.stop_kind = Some(kind);
        let turn_id = active.turn_id.clone();
        self.cancel_active();
        self.publish_snapshot(RuntimeSessionPhase::Cancelling {
            turn_id: turn_id.clone(),
        });
        Ok(turn_id)
    }

    fn cancel_active(&mut self) {
        if let Some(active) = &mut self.active {
            active.stop_kind.get_or_insert(TurnStopKind::Cancel);
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
        let outcome = match self.agent_tree_control.shutdown().await {
            Ok(()) => ShutdownOutcome::Complete,
            Err(error) => {
                log::warn!("failed to shut down session sub-agents: {error}");
                ShutdownOutcome::Failed
            }
        };
        self.client.drain_memory().await;
        let outcome = *self.shutdown_outcome.get_or_init(|| outcome);
        self.publish_snapshot(RuntimeSessionPhase::Closed);
        for waiter in self.shutdown_waiters.drain(..) {
            let _ = waiter.send(outcome.result());
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
            SessionCommand::StopTurn { response, .. } => {
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
