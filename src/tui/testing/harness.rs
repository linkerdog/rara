use std::io::{self, Write};

use futures::StreamExt;
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Rect, Size};
use tempfile::TempDir;

use super::FakeRuntimeClient;
use crate::config::ConfigManager;
use crate::runtime_control::{RuntimeControlEvent, SessionControlRequest};
use crate::tui::custom_terminal::Terminal;
use crate::tui::render;
use crate::tui::runtime::apply_tui_event;
use crate::tui::runtime_port::{
    RuntimeClientPort, RuntimeCommand, RuntimeEventStream, RuntimeProjectionEvent,
};
use crate::tui::state::{RuntimePhase, RuntimeSnapshot, TuiApp, TuiEvent};

const DEFAULT_WIDTH: u16 = 100;
const DEFAULT_HEIGHT: u16 = 30;

/// Drives the production TUI projection and renderer with scripted runtime
/// input. It intentionally has no wall-clock sleeps or real runtime objects.
pub(crate) struct TuiHarness {
    _config_dir: TempDir,
    app: TuiApp,
    runtime: FakeRuntimeClient,
    events: RuntimeEventStream,
    terminal: Terminal<TestBackendAdapter>,
}

impl TuiHarness {
    pub(crate) fn new(snapshot: RuntimeSnapshot) -> anyhow::Result<Self> {
        let config_dir = tempfile::tempdir()?;
        let mut app = TuiApp::new(ConfigManager {
            path: config_dir.path().join("config.json"),
        })?;
        app.snapshot = snapshot.clone();

        let runtime = FakeRuntimeClient::new(snapshot);
        let events = runtime.subscribe();
        let mut terminal = Terminal::new(TestBackendAdapter::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))?;
        terminal.set_viewport_area(Rect::new(0, 0, DEFAULT_WIDTH, DEFAULT_HEIGHT));

        Ok(Self {
            _config_dir: config_dir,
            app,
            runtime,
            events,
            terminal,
        })
    }

    pub(crate) async fn sync_snapshot(&mut self, snapshot: RuntimeSnapshot) {
        self.runtime.set_snapshot(snapshot);
        self.runtime.emit(RuntimeProjectionEvent::Snapshot(Box::new(
            self.runtime.snapshot().await.expect("fake snapshot"),
        )));
        self.pump_one().await;
    }

    pub(crate) async fn emit_runtime(&mut self, event: RuntimeControlEvent) {
        self.runtime
            .emit(RuntimeProjectionEvent::Runtime(Box::new(event)));
        self.pump_one().await;
    }

    pub(crate) async fn complete_turn(&mut self, reason: Option<String>) {
        self.runtime
            .emit(RuntimeProjectionEvent::Completed { reason });
        self.pump_one().await;
    }

    pub(crate) async fn cancel(&self) -> anyhow::Result<()> {
        self.runtime
            .send(RuntimeCommand::Session(
                SessionControlRequest::CancelCurrentTurn,
            ))
            .await
    }

    pub(crate) async fn disconnect(&mut self, reason: impl Into<String>) {
        self.runtime.disconnect(reason);
        self.pump_one().await;
    }

    pub(crate) async fn reconnect(&mut self) {
        self.runtime.reconnect();
        self.pump_one().await;
    }

    pub(crate) fn render(&mut self) -> anyhow::Result<()> {
        self.terminal
            .draw(|frame| render::render(frame, &mut self.app))?;
        Ok(())
    }

    pub(crate) fn expect_command(&self, expected: RuntimeCommand) {
        assert_eq!(self.runtime.commands(), [expected]);
    }

    pub(crate) fn expect_runtime_phase(&self, expected: RuntimePhase) {
        assert_eq!(self.app.runtime_phase, expected);
    }

    pub(crate) fn expect_transcript_contains(&self, expected: &str) {
        let found = self
            .app
            .committed_turns
            .iter()
            .chain(std::iter::once(&self.app.active_turn))
            .flat_map(|turn| turn.entries.iter())
            .any(|entry| entry.message.contains(expected));
        assert!(found, "transcript does not contain {expected:?}");
    }

    pub(crate) async fn pump_one(&mut self) {
        let event = self.events.next().await.expect("fake runtime event");
        match event {
            RuntimeProjectionEvent::Snapshot(snapshot) => self.app.snapshot = *snapshot,
            RuntimeProjectionEvent::Runtime(event) => {
                apply_tui_event(&mut self.app, TuiEvent::Runtime(event));
            }
            RuntimeProjectionEvent::Completed { reason } => {
                self.app.set_runtime_phase(RuntimePhase::Idle, reason);
            }
            RuntimeProjectionEvent::Disconnected { reason } => {
                self.app
                    .set_runtime_phase(RuntimePhase::Failed, Some(reason));
            }
            RuntimeProjectionEvent::Reconnected => {
                self.app.set_runtime_phase(RuntimePhase::Idle, None);
            }
        }
    }
}

struct TestBackendAdapter {
    inner: TestBackend,
}

impl TestBackendAdapter {
    fn new(width: u16, height: u16) -> Self {
        Self {
            inner: TestBackend::new(width, height),
        }
    }
}

impl Write for TestBackendAdapter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Backend for TestBackendAdapter {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        Backend::draw(&mut self.inner, content).map_err(|error| match error {})
    }

    fn append_lines(&mut self, line_count: u16) -> Result<(), Self::Error> {
        Backend::append_lines(&mut self.inner, line_count).map_err(|error| match error {})
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Backend::hide_cursor(&mut self.inner).map_err(|error| match error {})
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Backend::show_cursor(&mut self.inner).map_err(|error| match error {})
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Backend::get_cursor_position(&mut self.inner).map_err(|error| match error {})
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        Backend::set_cursor_position(&mut self.inner, position).map_err(|error| match error {})
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        Backend::clear(&mut self.inner).map_err(|error| match error {})
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        Backend::clear_region(&mut self.inner, clear_type).map_err(|error| match error {})
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Backend::size(&self.inner).map_err(|error| match error {})
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Backend::window_size(&mut self.inner).map_err(|error| match error {})
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Backend::flush(&mut self.inner).map_err(|error| match error {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_control::{AssistantEvent, RuntimeEvent, RuntimeProvenance, SessionEvent};

    fn session_event(event: SessionEvent) -> RuntimeControlEvent {
        RuntimeControlEvent {
            event_id: "event-1".into(),
            provenance: RuntimeProvenance::local_tui("test-session"),
            sequence: 1,
            event: RuntimeEvent::Session(event),
        }
    }

    fn assistant_event(event: AssistantEvent) -> RuntimeControlEvent {
        RuntimeControlEvent {
            event_id: "event-2".into(),
            provenance: RuntimeProvenance::local_tui("test-session"),
            sequence: 2,
            event: RuntimeEvent::Assistant(event),
        }
    }

    #[tokio::test]
    async fn harness_scripts_lifecycle_and_typed_cancel() {
        let mut harness = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
        let snapshot = RuntimeSnapshot {
            session_id: "scripted-session".into(),
            ..RuntimeSnapshot::default()
        };
        harness.sync_snapshot(snapshot).await;

        harness
            .emit_runtime(session_event(SessionEvent::Status {
                message: "Inspecting".into(),
            }))
            .await;
        harness.expect_runtime_phase(RuntimePhase::ProcessingResponse);
        harness
            .emit_runtime(assistant_event(AssistantEvent::Text("Inspecting".into())))
            .await;
        harness.expect_transcript_contains("Inspecting");
        harness
            .cancel()
            .await
            .expect("cancel command should be accepted");
        harness.expect_command(RuntimeCommand::Session(
            SessionControlRequest::CancelCurrentTurn,
        ));

        harness.complete_turn(None).await;
        harness.expect_runtime_phase(RuntimePhase::Idle);
        harness.disconnect("transport closed").await;
        harness.expect_runtime_phase(RuntimePhase::Failed);
        harness.reconnect().await;
        harness.expect_runtime_phase(RuntimePhase::Idle);
        harness.render().expect("render test backend");
    }
}
