use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};

use crate::agent::AgentEvent;
use crate::runtime_control::{
    ErrorEvent, RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, SessionEvent,
};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tui::state::TuiEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueryStopKind {
    Cancel,
    Interrupt,
}

#[cfg(test)]
#[path = "query_task/tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueryStopRequest {
    Requested,
    AlreadyRequested,
    Finished,
}

#[derive(Clone, Copy, Debug)]
enum QueryState {
    Running,
    Stopping(QueryStopKind),
    Finished(Option<QueryStopKind>),
}

/// Serializes stop admission with execution return for one compatibility query.
#[derive(Clone, Debug)]
pub(crate) struct QueryTaskControl {
    pub(crate) session_id: String,
    pub(crate) turn_id: String,
    state: Arc<Mutex<QueryState>>,
}

impl QueryTaskControl {
    pub(crate) fn new(session_id: String) -> Self {
        Self {
            session_id,
            turn_id: uuid::Uuid::new_v4().to_string(),
            state: Arc::new(Mutex::new(QueryState::Running)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, QueryState> {
        self.state.lock().unwrap_or_else(|poisoned| {
            log::warn!("query task control lock was poisoned; recovering");
            poisoned.into_inner()
        })
    }

    pub(crate) fn request_stop(&self, kind: QueryStopKind, token: &AtomicBool) -> QueryStopRequest {
        let mut state = self.lock();
        match *state {
            QueryState::Running => {
                *state = QueryState::Stopping(kind);
                token.store(true, Ordering::SeqCst);
                QueryStopRequest::Requested
            }
            QueryState::Stopping(_) => QueryStopRequest::AlreadyRequested,
            QueryState::Finished(_) => QueryStopRequest::Finished,
        }
    }

    pub(crate) fn stop_kind(&self) -> Option<QueryStopKind> {
        match *self.lock() {
            QueryState::Running => None,
            QueryState::Stopping(kind) => Some(kind),
            QueryState::Finished(kind) => kind,
        }
    }

    pub(crate) fn publish_event(
        &self,
        bus: &RuntimeEventBus,
        sender: &tokio::sync::mpsc::UnboundedSender<TuiEvent>,
        mut event: RuntimeControlEvent,
    ) {
        event.turn_id = Some(self.turn_id.clone());
        bus.publish_resequenced_control_event(event, |receipt| {
            if let Err(error) = sender.send(TuiEvent::Runtime(Box::new(receipt))) {
                log::warn!("query event receipt receiver closed: {error}");
            }
        });
    }

    /// Preserve intermediate diagnostics while task return owns dispatch's final error.
    pub(crate) fn publish_dispatch_event(
        &self,
        bus: &RuntimeEventBus,
        sender: &tokio::sync::mpsc::UnboundedSender<TuiEvent>,
        pending_error: &mut Option<RuntimeControlEvent>,
        event: RuntimeControlEvent,
    ) {
        // Dispatch emits a final error on Err and a final stop on Ok. Holding
        // only the latest diagnostic distinguishes that wrapper from errors
        // followed by more execution without classifying message strings.
        if let Some(error) = pending_error.take() {
            self.publish_event(bus, sender, error);
        }
        match &event.event {
            RuntimeEvent::Session(SessionEvent::TurnFinished { .. }) => {}
            RuntimeEvent::Error(ErrorEvent::RuntimeError {
                recoverable: false, ..
            }) => {
                *pending_error = Some(event);
            }
            _ => self.publish_event(bus, sender, event),
        }
    }

    pub(crate) fn publish_finished(
        &self,
        bus: &RuntimeEventBus,
        sender: &tokio::sync::mpsc::UnboundedSender<TuiEvent>,
        result: anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let execution_error = result.as_ref().err().map(|error| format!("{error:#}"));
        let (result, terminal) = self.finish(result);
        let provenance = RuntimeProvenance::local_tui(self.session_id.clone());
        let publish = |event| {
            self.publish_event(
                bus,
                sender,
                RuntimeControlEvent {
                    event_id: String::new(),
                    provenance: provenance.clone(),
                    turn_id: None,
                    sequence: 0,
                    event,
                },
            );
        };
        if let Some(message) = execution_error {
            bus.publish_raw(AgentEvent::AgentError {
                message: message.clone(),
                recoverable: false,
            });
            publish(RuntimeEvent::Error(ErrorEvent::RuntimeError {
                message,
                recoverable: false,
            }));
        }
        if !matches!(terminal, SessionEvent::TurnFailed { .. }) {
            let reason = match self.stop_kind() {
                Some(QueryStopKind::Cancel) => "cancelled by user",
                Some(QueryStopKind::Interrupt) => "interrupted by user",
                None => "turn complete",
            };
            bus.publish_raw(AgentEvent::AgentStop {
                reason: reason.into(),
            });
        }
        publish(RuntimeEvent::Session(terminal));
        result
    }

    /// Called only after execution returns, before publishing its final boundary.
    fn finish(&self, result: anyhow::Result<()>) -> (anyhow::Result<()>, SessionEvent) {
        let mut state = self.lock();
        let stop = match *state {
            QueryState::Running => None,
            QueryState::Stopping(kind) => Some(kind),
            QueryState::Finished(_) => unreachable!("query execution returns exactly once"),
        };
        *state = QueryState::Finished(stop);
        match stop {
            Some(kind) => {
                let (reason, terminal) = match kind {
                    QueryStopKind::Cancel => ("cancelled by user", SessionEvent::TurnCancelled),
                    QueryStopKind::Interrupt => {
                        ("interrupted by user", SessionEvent::TurnInterrupted)
                    }
                };
                let error = match result {
                    Ok(()) => anyhow::anyhow!(reason),
                    Err(error) => error.context(reason),
                };
                (Err(error), terminal)
            }
            None => {
                let terminal = match &result {
                    Ok(()) => SessionEvent::TurnFinished {
                        reason: Some("turn complete".into()),
                    },
                    Err(error) => SessionEvent::TurnFailed {
                        reason: format!("{error:#}"),
                    },
                };
                (result, terminal)
            }
        }
    }
}
