use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::{mpsc, oneshot, watch};

use super::command::{SessionCommand, TurnResultSender};
use super::shutdown::ShutdownOutcome;
use crate::{
    InputDiscardReason, PendingDisposition, PendingInteraction, RuntimeSessionError,
    RuntimeSessionId, RuntimeSessionPhase, RuntimeTurnId, RuntimeTurnOutcome, SessionDriver,
    SessionLifecycle, SessionSnapshot, SessionTurn, TurnContext, TurnFinishReason, TurnStopKind,
};

struct ActiveTurn {
    turn_id: RuntimeTurnId,
    generation: u64,
    cancellation: Arc<AtomicBool>,
    stop_kind: Option<TurnStopKind>,
    completed: TurnResultSender,
}

struct TurnCompletion<T> {
    turn_id: RuntimeTurnId,
    generation: u64,
    turn: T,
    result: anyhow::Result<()>,
}

pub(crate) struct SessionActor<D: SessionDriver> {
    id: RuntimeSessionId,
    driver: D,
    commands: mpsc::Receiver<SessionCommand<D>>,
    completions: mpsc::Sender<TurnCompletion<D::Turn>>,
    completion_receiver: mpsc::Receiver<TurnCompletion<D::Turn>>,
    snapshot: watch::Sender<SessionSnapshot<D::Pending>>,
    generation: u64,
    active: Option<ActiveTurn>,
    closing: bool,
    pending_input: Option<D::Pending>,
    shutdown_waiters: Vec<oneshot::Sender<Result<(), RuntimeSessionError>>>,
    shutdown_outcome: Arc<OnceLock<ShutdownOutcome>>,
}

impl<D: SessionDriver> SessionActor<D> {
    pub(crate) fn new(
        id: RuntimeSessionId,
        driver: D,
        commands: mpsc::Receiver<SessionCommand<D>>,
        snapshot: watch::Sender<SessionSnapshot<D::Pending>>,
        shutdown_outcome: Arc<OnceLock<ShutdownOutcome>>,
    ) -> Self {
        let (completions, completion_receiver) = mpsc::channel(1);
        Self {
            id,
            driver,
            commands,
            completions,
            completion_receiver,
            snapshot,
            generation: 0,
            active: None,
            closing: false,
            pending_input: None,
            shutdown_waiters: Vec::new(),
            shutdown_outcome,
        }
    }

    pub(crate) async fn run(mut self) {
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
                            if self.handle_command(command).await { return; }
                        }
                        None => {
                            self.begin_shutdown();
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

    async fn handle_command(&mut self, command: SessionCommand<D>) -> bool {
        if self.closing && !matches!(command, SessionCommand::Shutdown { .. }) {
            Self::reject_closed(command);
            return false;
        }
        match command {
            SessionCommand::StartTurn {
                turn_id,
                input,
                inference_agent,
                accepted,
                completed,
            } => {
                if let Err(error) = self.require_idle().and_then(|()| {
                    self.driver
                        .validate_input(&input, self.pending_input.as_ref())
                }) {
                    let _ = accepted.send(Err(error));
                    return false;
                }
                let pending = match &self.pending_input {
                    Some(_) if D::is_answer(&input) => PendingDisposition::Answered,
                    Some(_) => PendingDisposition::Superseded,
                    None => PendingDisposition::None,
                };
                let cancellation = Arc::new(AtomicBool::new(false));
                let context = TurnContext {
                    turn_id: turn_id.clone(),
                    cancellation: cancellation.clone(),
                    inference_agent,
                    pending,
                };
                let mut turn = match self.driver.begin_turn(input, context) {
                    Ok(turn) => turn,
                    Err(error) => {
                        let _ = accepted.send(Err(error));
                        return false;
                    }
                };
                if let Some(previous) = self.pending_input.take() {
                    let waiting_turn = previous.turn_id().clone();
                    match pending {
                        PendingDisposition::Answered => {
                            self.driver.publish(SessionLifecycle::InputAnswered {
                                waiting_turn,
                                turn_id: turn_id.clone(),
                            })
                        }
                        PendingDisposition::Superseded => {
                            self.driver.publish(SessionLifecycle::InputDiscarded {
                                waiting_turn,
                                reason: InputDiscardReason::Superseded,
                            })
                        }
                        PendingDisposition::None => {}
                    }
                }
                self.active = Some(ActiveTurn {
                    turn_id: turn_id.clone(),
                    generation: self.generation,
                    cancellation,
                    stop_kind: None,
                    completed,
                });
                self.driver.publish(SessionLifecycle::TurnStarted {
                    turn_id: turn_id.clone(),
                });
                self.publish_snapshot(RuntimeSessionPhase::Running {
                    turn_id: turn_id.clone(),
                });
                let completions = self.completions.clone();
                let generation = self.generation;
                tokio::spawn(async move {
                    let result = turn.run().await;
                    if completions
                        .send(TurnCompletion {
                            turn_id,
                            generation,
                            turn,
                            result,
                        })
                        .await
                        .is_err()
                    {
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
                let _ = response.send(self.stop_active(expected_turn, kind));
            }
            SessionCommand::Control { control, response } => {
                let result = match self.require_idle() {
                    Ok(()) => self.driver.control(control).await,
                    Err(error) => Err(error),
                };
                let _ = response.send(result);
            }
            SessionCommand::GetTranscript { response } => {
                let _ = response.send(self.require_idle().and_then(|()| self.driver.transcript()));
            }
            SessionCommand::ReplaceTranscript {
                transcript,
                response,
            } => {
                let result = if let Some(pending) = &self.pending_input {
                    Err(RuntimeSessionError::AwaitingInput {
                        waiting_turn: pending.turn_id().clone(),
                    })
                } else {
                    self.require_idle()
                        .and_then(|()| self.driver.replace_transcript(transcript))
                };
                let _ = response.send(result);
            }
            SessionCommand::QueryState { response } => {
                let mut snapshot = self.snapshot.borrow().clone();
                snapshot.last_sequence = self.driver.current_sequence();
                self.driver
                    .publish(SessionLifecycle::RuntimeState { snapshot });
                let _ = response.send(Ok(()));
            }
            SessionCommand::Shutdown { response } => {
                self.shutdown_waiters.push(response);
                self.begin_shutdown();
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

    fn require_idle(&self) -> Result<(), RuntimeSessionError> {
        match &self.active {
            Some(active) => Err(RuntimeSessionError::Busy {
                active_turn: active.turn_id.clone(),
            }),
            None => Ok(()),
        }
    }

    async fn finish_turn(&mut self, completion: TurnCompletion<D::Turn>) {
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
        let cancelled = active.cancellation.load(Ordering::SeqCst);
        let stop_kind = active.stop_kind.unwrap_or(TurnStopKind::Cancel);
        let discard = if self.closing {
            Some(InputDiscardReason::Shutdown)
        } else if cancelled {
            Some(match stop_kind {
                TurnStopKind::Cancel => InputDiscardReason::Cancelled,
                TurnStopKind::Interrupt => InputDiscardReason::Interrupted,
            })
        } else {
            None
        };
        let completed = self.driver.complete_turn(completion.turn, discard);
        if let Some(reason) = discard {
            if let Some(pending) = completed.pending_input {
                self.driver.publish(SessionLifecycle::InputDiscarded {
                    waiting_turn: pending.turn_id().clone(),
                    reason,
                });
            }
        } else {
            self.pending_input = completed.pending_input;
            if let Some(pending) = &self.pending_input {
                self.driver.publish(SessionLifecycle::InputRequested {
                    pending: pending.clone(),
                });
            }
        }
        self.driver.after_turn().await;
        let outcome = RuntimeTurnOutcome {
            turn_id: completion.turn_id.clone(),
            query_report: completed.query_report,
            transcript: completed.transcript,
        };
        let result = if cancelled {
            self.driver.publish(SessionLifecycle::TurnStopped {
                turn_id: completion.turn_id,
                kind: stop_kind,
            });
            match stop_kind {
                TurnStopKind::Cancel => Err(RuntimeSessionError::Cancelled { outcome }),
                TurnStopKind::Interrupt => Err(RuntimeSessionError::Interrupted { outcome }),
            }
        } else {
            match completion.result {
                Ok(()) => {
                    let reason = if self.pending_input.is_some() {
                        TurnFinishReason::AwaitingInput
                    } else {
                        TurnFinishReason::Completed
                    };
                    self.driver.publish(SessionLifecycle::TurnFinished {
                        turn_id: completion.turn_id,
                        reason,
                    });
                    Ok(outcome)
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    self.driver.publish(SessionLifecycle::TurnFailed {
                        turn_id: completion.turn_id,
                        message: message.clone(),
                    });
                    Err(RuntimeSessionError::Execution { message, outcome })
                }
            }
        };
        if !self.closing {
            let phase = match &self.pending_input {
                Some(pending) => RuntimeSessionPhase::AwaitingInput {
                    turn_id: pending.turn_id().clone(),
                },
                None => RuntimeSessionPhase::Idle,
            };
            self.publish_snapshot(phase);
        }
        let _ = active.completed.send(result);
    }

    fn stop_active(
        &mut self,
        expected_turn: Option<RuntimeTurnId>,
        kind: TurnStopKind,
    ) -> Result<RuntimeTurnId, RuntimeSessionError> {
        if self.active.is_none() {
            let pending = self
                .pending_input
                .as_ref()
                .ok_or(RuntimeSessionError::NotRunning)?;
            if let Some(expected) = expected_turn
                && &expected != pending.turn_id()
            {
                return Err(RuntimeSessionError::StaleInput {
                    expected,
                    waiting: pending.turn_id().clone(),
                });
            }
            let reason = match kind {
                TurnStopKind::Cancel => InputDiscardReason::Cancelled,
                TurnStopKind::Interrupt => InputDiscardReason::Interrupted,
            };
            let turn_id = self
                .discard_pending_input(reason)?
                .ok_or(RuntimeSessionError::NotRunning)?;
            self.cancel_active();
            self.publish_snapshot(RuntimeSessionPhase::Idle);
            return Ok(turn_id);
        }
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
        if let Err(error) = self.driver.cancel_descendants() {
            log::warn!("failed to cancel active sub-agents: {error}");
        }
    }

    fn discard_pending_input(
        &mut self,
        reason: InputDiscardReason,
    ) -> Result<Option<RuntimeTurnId>, RuntimeSessionError> {
        let Some(pending) = &self.pending_input else {
            return Ok(None);
        };
        let turn_id = pending.turn_id().clone();
        self.driver.discard_pending()?;
        self.pending_input = None;
        self.driver.publish(SessionLifecycle::InputDiscarded {
            waiting_turn: turn_id.clone(),
            reason,
        });
        Ok(Some(turn_id))
    }

    fn begin_shutdown(&mut self) {
        self.closing = true;
        if let Err(error) = self.discard_pending_input(InputDiscardReason::Shutdown) {
            log::warn!("failed to discard pending session input: {error}");
            self.shutdown_outcome
                .get_or_init(|| ShutdownOutcome::Failed);
        }
        if let Err(error) = self.driver.begin_shutdown() {
            log::warn!("failed to close sub-agent admission: {error}");
        }
        self.cancel_active();
        self.publish_snapshot(RuntimeSessionPhase::Closing);
    }

    async fn finish_shutdown(&mut self) {
        let outcome = match self.driver.shutdown().await {
            Ok(()) => ShutdownOutcome::Complete,
            Err(error) => {
                log::warn!("failed to shut down session resources: {error}");
                ShutdownOutcome::Failed
            }
        };
        let outcome = *self.shutdown_outcome.get_or_init(|| outcome);
        self.publish_snapshot(RuntimeSessionPhase::Closed);
        for waiter in self.shutdown_waiters.drain(..) {
            let _ = waiter.send(outcome.result());
        }
    }

    fn publish_snapshot(&self, phase: RuntimeSessionPhase) {
        self.snapshot.send_replace(SessionSnapshot {
            session_id: self.id.clone(),
            phase,
            generation: self.generation,
            last_sequence: self.driver.current_sequence(),
            pending_input: self.pending_input.clone(),
        });
    }

    fn reject_closed(command: SessionCommand<D>) {
        match command {
            SessionCommand::StartTurn { accepted, .. } => {
                let _ = accepted.send(Err(RuntimeSessionError::Closed));
            }
            SessionCommand::StopTurn { response, .. } => {
                let _ = response.send(Err(RuntimeSessionError::Closed));
            }
            SessionCommand::Control { response, .. }
            | SessionCommand::ReplaceTranscript { response, .. }
            | SessionCommand::QueryState { response }
            | SessionCommand::Shutdown { response } => {
                let _ = response.send(Err(RuntimeSessionError::Closed));
            }
            SessionCommand::GetTranscript { response } => {
                let _ = response.send(Err(RuntimeSessionError::Closed));
            }
        }
    }
}
