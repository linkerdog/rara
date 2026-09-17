use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, watch};

use super::shutdown::ShutdownOutcome;
use super::{RuntimeSession, RuntimeSessionError, RuntimeSessionId};

#[derive(Default)]
struct HostState {
    sessions: HashMap<RuntimeSessionId, RuntimeSession>,
    shutdown: Option<watch::Receiver<Option<ShutdownOutcome>>>,
}

/// Optional process-local registry for applications that host multiple sessions.
#[derive(Clone, Default)]
pub struct RuntimeHost {
    state: Arc<RwLock<HostState>>,
}

impl RuntimeHost {
    /// Create an empty, non-global runtime host.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a session, rejecting duplicates and incomplete host cleanup.
    pub async fn insert(&self, session: RuntimeSession) -> Result<(), RuntimeSessionError> {
        let mut state = self.state.write().await;
        if let Some(shutdown) = &state.shutdown {
            match *shutdown.borrow() {
                None => return Err(RuntimeSessionError::Closed),
                Some(ShutdownOutcome::Failed) => return Err(RuntimeSessionError::ShutdownFailed),
                Some(ShutdownOutcome::Complete) => {}
            }
        }
        if state.sessions.contains_key(session.id()) {
            return Err(RuntimeSessionError::AlreadyExists(session.id().clone()));
        }
        state.shutdown = None;
        state.sessions.insert(session.id().clone(), session);
        Ok(())
    }

    /// Resolve a cloneable session handle by identity.
    pub async fn get(&self, id: &RuntimeSessionId) -> Option<RuntimeSession> {
        self.state.read().await.sessions.get(id).cloned()
    }

    /// Shut down one session and release its identity only after cleanup succeeds.
    pub async fn remove(
        &self,
        id: &RuntimeSessionId,
    ) -> Result<Option<RuntimeSession>, RuntimeSessionError> {
        let session = self.get(id).await;
        if let Some(session) = &session {
            session.shutdown().await?;
            let mut state = self.state.write().await;
            // Another remover may have released this identity and admitted a new actor.
            if state
                .sessions
                .get(id)
                .is_some_and(|stored| stored.same_actor(session))
            {
                state.sessions.remove(id);
            }
        }
        Ok(session)
    }

    /// Return stable identities for all registered sessions.
    pub async fn session_ids(&self) -> Vec<RuntimeSessionId> {
        let mut ids = self
            .state
            .read()
            .await
            .sessions
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    /// Drain all registered sessions once, retaining failed cleanup for every caller.
    pub async fn shutdown(&self) -> Result<(), RuntimeSessionError> {
        let mut completion = {
            let mut state = self.state.write().await;
            if let Some(completion) = &state.shutdown {
                completion.clone()
            } else {
                let (sender, receiver) = watch::channel(None);
                state.shutdown = Some(receiver.clone());
                let sessions = state.sessions.values().cloned().collect::<Vec<_>>();
                let owner = self.state.clone();
                // The cleanup owner outlives any individual caller's cancellation.
                tokio::spawn(async move {
                    let results =
                        futures::future::join_all(sessions.into_iter().map(|session| async move {
                            let result = session.shutdown().await;
                            (session, result)
                        }))
                        .await;
                    let mut state = owner.write().await;
                    let mut outcome = ShutdownOutcome::Complete;
                    for (session, result) in results {
                        match result {
                            Ok(()) => {
                                if state
                                    .sessions
                                    .get(session.id())
                                    .is_some_and(|stored| stored.same_actor(&session))
                                {
                                    state.sessions.remove(session.id());
                                }
                            }
                            Err(error) => {
                                log::warn!(
                                    "failed to shut down hosted session {}: {error}",
                                    session.id()
                                );
                                outcome = ShutdownOutcome::Failed;
                            }
                        }
                    }
                    sender.send_replace(Some(outcome));
                });
                receiver
            }
        };
        loop {
            if let Some(outcome) = *completion.borrow() {
                return outcome.result();
            }
            completion
                .changed()
                .await
                .map_err(|_| RuntimeSessionError::ActorStopped)?;
        }
    }
}
