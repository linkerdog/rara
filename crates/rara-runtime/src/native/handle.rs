use std::sync::{Arc, OnceLock};

use rara_core::llm::types::Message;
use rara_observability::InferenceTask;
use tokio::sync::{mpsc, oneshot, watch};

use super::actor::SessionActor;
use super::command::SessionCommand;
use super::shutdown::ShutdownOutcome;
use crate::{
    RuntimeSessionError, RuntimeSessionId, RuntimeSessionPhase, RuntimeTurn, RuntimeTurnId,
    SessionDriver, SessionSnapshot, TurnStopKind,
};

/// Cloneable ownership handle shared by native and embedding adapters.
pub struct SessionHandle<D: SessionDriver> {
    id: RuntimeSessionId,
    commands: mpsc::Sender<SessionCommand<D>>,
    snapshot: watch::Receiver<SessionSnapshot<D::Pending>>,
    shutdown_outcome: Arc<OnceLock<ShutdownOutcome>>,
}

impl<D: SessionDriver> Clone for SessionHandle<D> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            commands: self.commands.clone(),
            snapshot: self.snapshot.clone(),
            shutdown_outcome: self.shutdown_outcome.clone(),
        }
    }
}

impl<D: SessionDriver> SessionHandle<D> {
    /// Transfer one driver into a session actor on the current Tokio runtime.
    pub fn start(id: RuntimeSessionId, driver: D, command_capacity: usize) -> Self {
        let initial = SessionSnapshot {
            session_id: id.clone(),
            phase: RuntimeSessionPhase::Idle,
            generation: 0,
            last_sequence: driver.current_sequence(),
            pending_input: None,
        };
        let (snapshot_sender, snapshot) = watch::channel(initial);
        let (commands, receiver) = mpsc::channel(command_capacity.max(1));
        let shutdown_outcome = Arc::new(OnceLock::new());
        tokio::spawn(
            SessionActor::new(
                id.clone(),
                driver,
                receiver,
                snapshot_sender,
                shutdown_outcome.clone(),
            )
            .run(),
        );
        Self {
            id,
            commands,
            snapshot,
            shutdown_outcome,
        }
    }

    pub fn id(&self) -> &RuntimeSessionId {
        &self.id
    }

    pub fn same_actor(&self, other: &Self) -> bool {
        self.commands.same_channel(&other.commands)
    }

    pub fn snapshot(&self) -> SessionSnapshot<D::Pending> {
        self.snapshot.borrow().clone()
    }

    pub fn subscribe_snapshots(&self) -> watch::Receiver<SessionSnapshot<D::Pending>> {
        self.snapshot.clone()
    }

    pub async fn submit(
        &self,
        input: D::Input,
        accounting: InferenceTask,
    ) -> Result<RuntimeTurn, RuntimeSessionError> {
        let turn_id = RuntimeTurnId::generate();
        let (accepted, admission) = oneshot::channel();
        let (completed, completion) = oneshot::channel();
        self.try_send(SessionCommand::StartTurn {
            turn_id: turn_id.clone(),
            input,
            inference_agent: accounting.start_agent(None),
            accepted,
            completed,
        })?;
        admission
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)??;
        Ok(RuntimeTurn::new(turn_id, completion, accounting))
    }

    pub async fn stop_turn(
        &self,
        expected_turn: Option<RuntimeTurnId>,
        kind: TurnStopKind,
    ) -> Result<RuntimeTurnId, RuntimeSessionError> {
        let (response, receiver) = oneshot::channel();
        self.try_send(SessionCommand::StopTurn {
            expected_turn,
            kind,
            response,
        })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    pub async fn control(&self, control: D::Control) -> Result<(), RuntimeSessionError> {
        let (response, receiver) = oneshot::channel();
        self.try_send(SessionCommand::Control { control, response })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    pub async fn transcript(&self) -> Result<Vec<Message>, RuntimeSessionError> {
        let (response, receiver) = oneshot::channel();
        self.try_send(SessionCommand::GetTranscript { response })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    pub async fn replace_transcript(
        &self,
        transcript: Vec<Message>,
    ) -> Result<(), RuntimeSessionError> {
        let (response, receiver) = oneshot::channel();
        self.try_send(SessionCommand::ReplaceTranscript {
            transcript,
            response,
        })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    pub async fn query_runtime_state(&self) -> Result<(), RuntimeSessionError> {
        let (response, receiver) = oneshot::channel();
        self.try_send(SessionCommand::QueryState { response })?;
        receiver
            .await
            .map_err(|_| RuntimeSessionError::ActorStopped)?
    }

    /// Share a durable cleanup receipt, even if another caller drops its wait.
    pub async fn shutdown(&self) -> Result<(), RuntimeSessionError> {
        if matches!(self.snapshot().phase, RuntimeSessionPhase::Closed) {
            return self.shutdown_result();
        }
        if matches!(self.snapshot().phase, RuntimeSessionPhase::Closing) {
            return self.wait_until_closed().await;
        }
        let (response, receiver) = oneshot::channel();
        if self
            .commands
            .send(SessionCommand::Shutdown { response })
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

    fn try_send(&self, command: SessionCommand<D>) -> Result<(), RuntimeSessionError> {
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
