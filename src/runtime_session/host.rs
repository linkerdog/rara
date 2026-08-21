use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::{RuntimeSession, RuntimeSessionError, RuntimeSessionId};

/// Optional process-local registry for applications that host multiple sessions.
#[derive(Clone, Default)]
pub struct RuntimeHost {
    sessions: Arc<RwLock<HashMap<RuntimeSessionId, RuntimeSession>>>,
}

impl RuntimeHost {
    /// Create an empty, non-global runtime host.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a session and reject duplicate identities.
    pub async fn insert(&self, session: RuntimeSession) -> Result<(), RuntimeSessionError> {
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(session.id()) {
            return Err(RuntimeSessionError::AlreadyExists(session.id().clone()));
        }
        sessions.insert(session.id().clone(), session);
        Ok(())
    }

    /// Resolve a cloneable session handle by identity.
    pub async fn get(&self, id: &RuntimeSessionId) -> Option<RuntimeSession> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Remove and explicitly shut down one session.
    pub async fn remove(
        &self,
        id: &RuntimeSessionId,
    ) -> Result<Option<RuntimeSession>, RuntimeSessionError> {
        let session = self.sessions.write().await.remove(id);
        if let Some(session) = &session {
            session.shutdown().await?;
        }
        Ok(session)
    }

    /// Return stable identities for all registered sessions.
    pub async fn session_ids(&self) -> Vec<RuntimeSessionId> {
        let mut ids = self
            .sessions
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    /// Stop and remove every registered session.
    pub async fn shutdown(&self) -> Result<(), RuntimeSessionError> {
        let mut sessions = {
            let mut stored = self.sessions.write().await;
            stored.drain().collect::<Vec<_>>()
        };
        sessions.sort_by(|(left, _), (right, _)| left.cmp(right));
        let results = futures::future::join_all(
            sessions
                .into_iter()
                .map(|(_, session)| async move { session.shutdown().await }),
        )
        .await;
        results
            .into_iter()
            .find_map(Result::err)
            .map_or(Ok(()), Err)
    }
}
