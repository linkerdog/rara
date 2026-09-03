use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::tui::runtime_port::{
    RuntimeClientPort, RuntimeCommand, RuntimeEventStream, RuntimeProjectionEvent,
};
use crate::tui::state::RuntimeSnapshot;

const EVENT_BUFFER: usize = 64;

/// A deterministic runtime port for TUI tests.
///
/// The fake owns only the port contract. Tests control snapshots, projected
/// events, lifecycle transitions, and captured commands without constructing
/// an `Agent`, registry, or task handle.
#[derive(Clone)]
pub(crate) struct FakeRuntimeClient {
    snapshot: Arc<Mutex<RuntimeSnapshot>>,
    commands: Arc<Mutex<Vec<RuntimeCommand>>>,
    events: broadcast::Sender<RuntimeProjectionEvent>,
    connected: Arc<Mutex<bool>>,
}

impl FakeRuntimeClient {
    pub(crate) fn new(snapshot: RuntimeSnapshot) -> Self {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Self {
            snapshot: Arc::new(Mutex::new(snapshot)),
            commands: Arc::new(Mutex::new(Vec::new())),
            events,
            connected: Arc::new(Mutex::new(true)),
        }
    }

    pub(crate) fn set_snapshot(&self, snapshot: RuntimeSnapshot) {
        *self.snapshot.lock().expect("snapshot lock") = snapshot;
    }

    pub(crate) fn emit(&self, event: RuntimeProjectionEvent) {
        let _ = self.events.send(event);
    }

    pub(crate) fn disconnect(&self, reason: impl Into<String>) {
        *self.connected.lock().expect("connection lock") = false;
        self.emit(RuntimeProjectionEvent::Disconnected {
            reason: reason.into(),
        });
    }

    pub(crate) fn reconnect(&self) {
        *self.connected.lock().expect("connection lock") = true;
        self.emit(RuntimeProjectionEvent::Reconnected);
    }

    pub(crate) fn commands(&self) -> Vec<RuntimeCommand> {
        self.commands.lock().expect("command lock").clone()
    }
}

#[async_trait]
impl RuntimeClientPort for FakeRuntimeClient {
    async fn snapshot(&self) -> anyhow::Result<RuntimeSnapshot> {
        Ok(self.snapshot.lock().expect("snapshot lock").clone())
    }

    async fn send(&self, command: RuntimeCommand) -> anyhow::Result<()> {
        if !*self.connected.lock().expect("connection lock") {
            anyhow::bail!("fake runtime is disconnected");
        }
        self.commands.lock().expect("command lock").push(command);
        Ok(())
    }

    fn publish_snapshot(&self, snapshot: RuntimeSnapshot) {
        self.set_snapshot(snapshot);
    }

    fn subscribe(&self) -> RuntimeEventStream {
        let receiver = self.events.subscribe();
        Box::pin(
            BroadcastStream::new(receiver).filter_map(|event| async move {
                match event {
                    Ok(event) => Some(event),
                    Err(error) => {
                        log::warn!("fake runtime event stream lagged: {error}");
                        None
                    }
                }
            }),
        )
    }
}
