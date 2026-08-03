//! The narrow runtime contract consumed by the TUI controller.
//!
//! The in-process runtime still uses the compatibility task bridge today. This
//! contract is the seam for a future app-server client and the test-only fake;
//! it must not expose `Agent`, registries, or task join handles.

use std::pin::Pin;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use futures::Stream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::runtime_control::{
    ApprovalControlRequest, InputControlRequest, RuntimeControlEvent, SessionControlRequest,
};
use crate::runtime_event_bus::RuntimeEventBus;
use crate::tui::state::RuntimeSnapshot;

// Contract items are intentionally ahead of their adapters; the next
// in-process and scripted implementations will consume them.
#[allow(dead_code)]
pub(crate) type RuntimeEventStream = Pin<Box<dyn Stream<Item = RuntimeProjectionEvent> + Send>>;

// Contract items are intentionally ahead of their adapters; the next
// in-process and scripted implementations will consume them.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RuntimeCommand {
    Session(SessionControlRequest),
    Input(InputControlRequest),
    Approval(ApprovalControlRequest),
    Maintenance(RuntimeMaintenanceCommand),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeMaintenanceCommand {
    Compact,
    Rebuild,
    LoadDeepSeekModels,
    LoadKimiModels,
}

// Contract items are intentionally ahead of their adapters; the next
// in-process and scripted implementations will consume them.
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) enum RuntimeProjectionEvent {
    Snapshot(Box<RuntimeSnapshot>),
    Runtime(Box<RuntimeControlEvent>),
    Completed { reason: Option<String> },
    Disconnected { reason: String },
    Reconnected,
}

/// Runtime capabilities required by the TUI controller.
///
/// Implementations own execution state and transport details. They must not
/// require the controller to know about agents, registries, or task handles.
// Contract items are intentionally ahead of their adapters; the next
// in-process and scripted implementations will consume them.
#[allow(dead_code)]
#[async_trait]
pub(crate) trait RuntimeClientPort: Send {
    async fn snapshot(&self) -> anyhow::Result<RuntimeSnapshot>;
    async fn send(&self, command: RuntimeCommand) -> anyhow::Result<()>;
    fn subscribe(&self) -> RuntimeEventStream;
}

/// In-process adapter for the session runtime event bus.
///
/// Runtime execution still uses the compatibility task bridge for commands
/// and completion. This adapter makes the structured event and snapshot path
/// identical for the in-process TUI and future app-server clients.
pub(crate) struct InProcessRuntimeClientPort {
    event_bus: Arc<RuntimeEventBus>,
    snapshot: Arc<RwLock<RuntimeSnapshot>>,
    command_sender: UnboundedSender<RuntimeCommand>,
}

impl InProcessRuntimeClientPort {
    pub(crate) fn new(
        event_bus: Arc<RuntimeEventBus>,
        snapshot: Arc<RwLock<RuntimeSnapshot>>,
    ) -> (Self, UnboundedReceiver<RuntimeCommand>) {
        let (command_sender, command_receiver) = unbounded_channel();
        (
            Self {
                event_bus,
                snapshot,
                command_sender,
            },
            command_receiver,
        )
    }

    pub(crate) fn snapshot_store(&self) -> Arc<RwLock<RuntimeSnapshot>> {
        self.snapshot.clone()
    }
}

#[async_trait]
impl RuntimeClientPort for InProcessRuntimeClientPort {
    async fn snapshot(&self) -> anyhow::Result<RuntimeSnapshot> {
        self.snapshot
            .read()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| anyhow::anyhow!("runtime snapshot lock is poisoned"))
    }

    async fn send(&self, command: RuntimeCommand) -> anyhow::Result<()> {
        self.command_sender
            .send(command)
            .map_err(|_| anyhow::anyhow!("in-process runtime command channel is closed"))
    }

    fn subscribe(&self) -> RuntimeEventStream {
        let receiver = self.event_bus.subscribe_control();
        Box::pin(futures::stream::unfold(
            receiver,
            |mut receiver| async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            return Some((
                                RuntimeProjectionEvent::Runtime(Box::new(event)),
                                receiver,
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures::{StreamExt, stream};

    use super::{
        InProcessRuntimeClientPort, RuntimeClientPort, RuntimeCommand, RuntimeEventStream,
        RuntimeProjectionEvent,
    };
    use crate::agent::AgentEvent;
    use crate::runtime_control::RuntimeProvenance;
    use crate::runtime_control::SessionControlRequest;
    use crate::runtime_event_bus::RuntimeEventBus;
    use crate::tui::state::RuntimeSnapshot;

    struct FakePort {
        commands: Arc<Mutex<Vec<RuntimeCommand>>>,
    }

    #[async_trait]
    impl RuntimeClientPort for FakePort {
        async fn snapshot(&self) -> anyhow::Result<RuntimeSnapshot> {
            Ok(RuntimeSnapshot::default())
        }

        async fn send(&self, command: RuntimeCommand) -> anyhow::Result<()> {
            self.commands
                .lock()
                .expect("command log lock")
                .push(command);
            Ok(())
        }

        fn subscribe(&self) -> RuntimeEventStream {
            Box::pin(stream::empty())
        }
    }

    #[tokio::test]
    async fn fake_port_can_capture_typed_commands_without_runtime_objects() {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let port = FakePort {
            commands: commands.clone(),
        };

        port.send(RuntimeCommand::Session(
            SessionControlRequest::CancelCurrentTurn,
        ))
        .await
        .expect("send command");

        assert!(matches!(
            commands.lock().expect("command log lock").as_slice(),
            [RuntimeCommand::Session(
                SessionControlRequest::CancelCurrentTurn
            )]
        ));
    }

    #[tokio::test]
    async fn in_process_port_projects_control_bus_events() {
        let bus = std::sync::Arc::new(RuntimeEventBus::new(8));
        let (port, _commands) = InProcessRuntimeClientPort::new(
            bus.clone(),
            std::sync::Arc::new(std::sync::RwLock::new(RuntimeSnapshot::default())),
        );
        let mut events = port.subscribe();

        bus.send_with_provenance(
            AgentEvent::Status("ready".into()),
            RuntimeProvenance::local_tui("test-session"),
        );

        assert!(matches!(
            events.next().await,
            Some(RuntimeProjectionEvent::Runtime(event))
                if event.sequence == 1
        ));
    }
}
