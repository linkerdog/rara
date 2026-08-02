//! The narrow runtime contract consumed by the TUI controller.
//!
//! The in-process runtime still uses the compatibility task bridge today. This
//! contract is the seam for a future app-server client and the test-only fake;
//! it must not expose `Agent`, registries, or task join handles.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::runtime_control::{
    ApprovalControlRequest, InputControlRequest, RuntimeControlEvent, SessionControlRequest,
};
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures::stream;

    use super::{RuntimeClientPort, RuntimeCommand, RuntimeEventStream};
    use crate::runtime_control::SessionControlRequest;
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
}
