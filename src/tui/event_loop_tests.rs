use std::cell::{Cell, RefCell};
use std::io;
use std::rc::Rc;
use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::poll;
use tokio::sync::mpsc;
use tokio::time::{Duration, advance};

use super::{EventSource, run_event_loop};
use crate::config::{ConfigManager, RaraConfig};
use crate::llm::{LlmBackend, LlmResponse, Message};
use crate::oauth::OAuthManager;
use crate::runtime_client::RuntimeClient;
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};
use crate::runtime_control::{
    AssistantEvent, InputControlRequest, RuntimeControlEvent, RuntimeEvent, RuntimeProvenance,
    SessionEvent,
};
use crate::tui::controller::TuiController;
use crate::tui::custom_terminal::Terminal;
use crate::tui::message_role::MessageRole;
use crate::tui::runtime::RuntimeCommandProcessor;
use crate::tui::runtime_port::{RuntimeCommand, RuntimeProjectionEvent};
use crate::tui::state::{
    PermissionMode, QuitShortcutKey, RunningTask, TaskCompletion, TaskKind, TuiApp,
};
use crate::tui::testing::FakeRuntimeClient;
use crate::tui::testing::terminal_emulator::{EmulatedScreen, EmulatorBackend};

struct UnusedBackend;

#[async_trait::async_trait]
impl LlmBackend for UnusedBackend {
    async fn ask(&self, _: &[Message], _: &[serde_json::Value]) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("loop fixture must not call a provider")
    }

    async fn summarize(&self, _: &[Message], _: &str) -> anyhow::Result<String> {
        anyhow::bail!("loop fixture must not call a provider")
    }
}

#[derive(Default)]
struct SourceProbe {
    maintenance_ticks: Cell<usize>,
    fail_maintenance: Cell<bool>,
    editor_seeds: RefCell<Vec<String>>,
    editor_result: RefCell<Option<tokio::sync::oneshot::Receiver<anyhow::Result<String>>>>,
    #[cfg(unix)]
    suspends: Cell<usize>,
}

struct FakeEventSource {
    receiver: mpsc::UnboundedReceiver<io::Result<Event>>,
    probe: Rc<SourceProbe>,
}

impl EventSource<EmulatorBackend> for FakeEventSource {
    async fn next_event(&mut self) -> Option<io::Result<Event>> {
        self.receiver.recv().await
    }

    fn maintain_raw_mode(&mut self) -> io::Result<()> {
        self.probe
            .maintenance_ticks
            .set(self.probe.maintenance_ticks.get() + 1);
        if self.probe.fail_maintenance.get() {
            return Err(io::Error::other("injected mode maintenance failure"));
        }
        Ok(())
    }

    async fn edit_external(
        &mut self,
        terminal: &mut Terminal<EmulatorBackend>,
        request: crate::tui::external_editor::EditorRequest,
    ) -> io::Result<anyhow::Result<String>> {
        terminal.finish_inline_viewport()?;
        self.probe
            .editor_seeds
            .borrow_mut()
            .push(request.seed.to_string());
        let receiver = self.probe.editor_result.borrow_mut().take();
        match receiver {
            Some(receiver) => Ok(receiver.await.map_err(io::Error::other)?),
            None => Ok(Err(anyhow::anyhow!("no scripted editor result"))),
        }
    }

    #[cfg(unix)]
    fn suspend(&mut self, terminal: &mut Terminal<EmulatorBackend>) -> io::Result<()> {
        self.probe.suspends.set(self.probe.suspends.get() + 1);
        terminal.finish_inline_viewport()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    controller: TuiController,
    processor: RuntimeCommandProcessor,
    port: Arc<FakeRuntimeClient>,
    commands: mpsc::UnboundedSender<RuntimeCommand>,
    oauth: Arc<OAuthManager>,
    terminal: Terminal<EmulatorBackend>,
    source: FakeEventSource,
    input: mpsc::UnboundedSender<io::Result<Event>>,
    screen: Rc<RefCell<EmulatedScreen>>,
}

async fn fixture_runtime(root: &std::path::Path) -> (RaraConfig, RuntimeClient) {
    let mut config = RaraConfig::default();
    config.builtin_plugins.nowledge_mem.enabled = false;
    let options = RuntimeBootstrapOptions::with_plugin_dirs(Vec::new())
        .with_rara_home(Some(root.join("state")))
        .with_backend(Some(Arc::new(UnusedBackend)))
        .with_tool_manager(Some(rara_tools::tool::ToolManager::new()))
        .with_extension_discovery(false)
        .with_memory_facilities(false)
        .with_transcript_persistence(false);
    let bootstrap =
        initialize_rara_context_for_workspace_with_options(&config, Some(root), None, options)
            .await
            .unwrap();
    let runtime = RuntimeClient::from_bootstrap(bootstrap).await;
    (config, runtime)
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (config, runtime) = fixture_runtime(dir.path()).await;
        let mut app = TuiApp::with_config(
            ConfigManager {
                path: dir.path().join("config.json"),
            },
            config,
        )
        .unwrap();
        app.goal_handle = runtime.goal_handle.clone();
        app.event_bus = Some(runtime.event_bus.clone());
        app.mcp_manager = Some(runtime.mcp_manager.clone());
        app.memory_handler = Some(Arc::new(
            crate::protocol_sources::MemoryControlHandler::with_store(
                runtime.event_bus.clone(),
                runtime.agent().unwrap().memory_store.clone(),
            ),
        ));
        let port = Arc::new(FakeRuntimeClient::new(app.snapshot.clone().into_inner()));
        let (commands, receiver) = mpsc::unbounded_channel();
        let controller = TuiController::new(app, port.clone(), receiver);
        let oauth = Arc::new(OAuthManager::new_for_config_dir(dir.path().join("oauth")).unwrap());
        let backend = EmulatorBackend::new(30, 100);
        let screen = backend.screen.clone();
        let (input, receiver) = mpsc::unbounded_channel();
        Self {
            _dir: dir,
            controller,
            processor: RuntimeCommandProcessor::new(runtime),
            port,
            commands,
            oauth,
            terminal: Terminal::new(backend).unwrap(),
            source: FakeEventSource {
                receiver,
                probe: Rc::new(SourceProbe::default()),
            },
            input,
            screen,
        }
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        run_event_loop(
            &mut self.terminal,
            &mut self.controller,
            &mut self.processor,
            &self.oauth,
            &mut self.source,
            super::StartupMaintenance::None,
        )
        .await
    }
}

fn frame_count(screen: &Rc<RefCell<EmulatedScreen>>) -> usize {
    screen
        .borrow()
        .output
        .windows(b"\x1b[?2026l".len())
        .filter(|bytes| *bytes == b"\x1b[?2026l")
        .count()
}

fn emit(port: &FakeRuntimeClient, sequence: u64, event: RuntimeEvent) {
    port.emit(RuntimeProjectionEvent::Runtime(Box::new(
        RuntimeControlEvent {
            event_id: format!("event-{sequence}"),
            provenance: RuntimeProvenance::local_tui("loop-session"),
            turn_id: Some("loop-turn".into()),
            sequence,
            event,
        },
    )));
}

#[tokio::test]
async fn bursts_preserve_input_and_runtime_order_and_wake_one_trailing_frame() {
    let mut fixture = Fixture::new().await;
    fixture
        .controller
        .app_mut()
        .push_entry(MessageRole::User, "Stream the ordered tokens.");
    let input = fixture.input.clone();
    let port = fixture.port.clone();
    let screen = fixture.screen.clone();
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 1);
        emit(&port, 1, RuntimeEvent::Session(SessionEvent::TurnStarted));
        for (index, text) in ["First. ", "Second. ", "Final."].into_iter().enumerate() {
            emit(
                &port,
                index as u64 + 2,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta(text.into())),
            );
        }
        input
            .send(Ok(Event::Paste("draft\nsecond line".into())))
            .unwrap();
        input
            .send(Ok(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            ))))
            .unwrap();
        assert!(poll!(&mut future).is_pending());
        assert_eq!(
            port.commands(),
            [RuntimeCommand::Input(
                InputControlRequest::SubmitUserPrompt {
                    prompt: "draft\nsecond line".into()
                }
            )]
        );
        assert_eq!(frame_count(&screen), 1, "burst must not render per event");
        advance(Duration::from_millis(16)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 1, "frame rate limit must be honored");
        advance(Duration::from_millis(2)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(
            frame_count(&screen),
            2,
            "frame deadline must wake without input or maintenance"
        );
        let contents = screen.borrow().parser.screen().contents();
        assert!(contents.contains("First. Second. Final."), "{contents}");
        advance(Duration::from_millis(500)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(
            frame_count(&screen),
            2,
            "consumed dirty state must not repaint forever"
        );
    }
    assert!(fixture.controller.app().bottom_pane.input.is_empty());
    assert!(
        fixture
            .controller
            .app()
            .bottom_pane
            .large_paste_pending
            .is_empty()
    );
    // A final Text event could overwrite missing/reordered deltas and hide a defect.
    assert_eq!(
        fixture
            .controller
            .app()
            .agent_markdown_stream
            .as_ref()
            .unwrap()
            .raw_text,
        "First. Second. Final."
    );
}

#[tokio::test]
async fn idle_maintenance_and_released_keys_do_not_repaint_but_focus_does() {
    let mut fixture = Fixture::new().await;
    let input = fixture.input.clone();
    let screen = fixture.screen.clone();
    let probe = fixture.source.probe.clone();
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        let initial_ticks = probe.maintenance_ticks.get();
        advance(Duration::from_millis(166)).await;
        assert!(poll!(&mut future).is_pending());
        assert!(probe.maintenance_ticks.get() > initial_ticks);
        input
            .send(Ok(Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            ))))
            .unwrap();
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 1);
        input.send(Ok(Event::FocusLost)).unwrap();
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 2);
    }
    assert!(!fixture.controller.app().terminal_focused);
    assert!(fixture.controller.app().bottom_pane.input.is_empty());
}

#[tokio::test]
async fn maintenance_expiration_requests_a_frame_without_input() {
    let mut fixture = Fixture::new().await;
    fixture.controller.app_mut().quit_shortcut.press(
        QuitShortcutKey::CtrlC,
        std::time::Instant::now() - Duration::from_secs(2),
    );
    let screen = fixture.screen.clone();
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 1);
        advance(Duration::from_millis(18)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(
            frame_count(&screen),
            2,
            "maintenance changes need a trailing frame"
        );
    }
    assert_eq!(fixture.controller.app().quit_shortcut.key(), None);
}

#[tokio::test]
async fn resize_burst_repaints_stale_cells_and_uses_backend_dimensions() {
    let mut fixture = Fixture::new().await;
    let input = fixture.input.clone();
    let screen = fixture.screen.clone();
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        {
            let mut screen = screen.borrow_mut();
            screen.parser.screen_mut().set_size(20, 80);
            screen.parser.screen_mut().set_size(30, 100);
            screen.parser.process(b"\x1b[1;1HGHOST");
        }
        input.send(Ok(Event::Resize(80, 20))).unwrap();
        input.send(Ok(Event::Resize(100, 30))).unwrap();
        assert!(poll!(&mut future).is_pending());
        advance(Duration::from_millis(18)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 2);
        assert!(!screen.borrow().parser.screen().contents().contains("GHOST"));
        screen.borrow_mut().parser.screen_mut().set_size(18, 60);
        input.send(Ok(Event::Resize(60, 18))).unwrap();
        assert!(poll!(&mut future).is_pending());
        advance(Duration::from_millis(18)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 3);
    }
    assert_eq!(fixture.controller.app().terminal_width, 60);
    assert_eq!(
        fixture.terminal.viewport_area,
        ratatui::layout::Rect::new(0, 0, 60, 18)
    );
}

#[tokio::test]
async fn input_error_is_visible_and_eof_ends_the_loop() {
    let mut fixture = Fixture::new().await;
    let screen = fixture.screen.clone();
    tokio::time::pause();
    let future = run_event_loop(
        &mut fixture.terminal,
        &mut fixture.controller,
        &mut fixture.processor,
        &fixture.oauth,
        &mut fixture.source,
        super::StartupMaintenance::None,
    );
    tokio::pin!(future);
    assert!(poll!(&mut future).is_pending());
    fixture
        .input
        .send(Err(io::Error::other("scripted input failure")))
        .unwrap();
    assert!(poll!(&mut future).is_pending());
    advance(Duration::from_millis(18)).await;
    assert!(poll!(&mut future).is_pending());
    assert!(
        screen
            .borrow()
            .parser
            .screen()
            .contents()
            .contains("scripted input failure")
    );
    drop(fixture.input);
    future.await.unwrap();
}

#[tokio::test]
async fn terminal_draw_and_maintenance_errors_end_the_session() {
    let mut fixture = Fixture::new().await;
    tokio::time::pause();
    fixture.screen.borrow_mut().fail_next_write = true;
    let error = fixture.run().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected terminal write failure")
    );

    let mut fixture = Fixture::new().await;
    fixture.source.probe.fail_maintenance.set(true);
    let error = fixture.run().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected mode maintenance failure")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn suspend_routes_through_the_owner_and_reserves_a_new_frame() {
    let mut fixture = Fixture::new().await;
    let input = fixture.input.clone();
    let screen = fixture.screen.clone();
    let probe = fixture.source.probe.clone();
    tokio::time::pause();
    let future = fixture.run();
    tokio::pin!(future);
    assert!(poll!(&mut future).is_pending());
    input
        .send(Ok(Event::Key(KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::CONTROL,
        ))))
        .unwrap();
    assert!(poll!(&mut future).is_pending());
    assert_eq!(probe.suspends.get(), 1);
    advance(Duration::from_millis(18)).await;
    assert!(poll!(&mut future).is_pending());
    assert_eq!(frame_count(&screen), 2);
}

#[tokio::test]
async fn runtime_command_is_applied_and_painted_through_the_processor() {
    let mut fixture = Fixture::new().await;
    let commands = fixture.commands.clone();
    let screen = fixture.screen.clone();
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        commands
            .send(RuntimeCommand::SetPermissionMode(PermissionMode::ReadOnly))
            .unwrap();
        assert!(poll!(&mut future).is_pending());
        advance(Duration::from_millis(18)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 2);
        let contents = screen.borrow().parser.screen().contents();
        assert!(
            contents.contains("read-only planning; approve to execute"),
            "{contents}"
        );
    }
    assert_eq!(
        fixture.controller.app().permission_mode,
        PermissionMode::ReadOnly
    );
}

#[tokio::test]
async fn joined_task_completion_is_consumed_and_painted_without_input() {
    let mut fixture = Fixture::new().await;
    fixture
        .controller
        .app_mut()
        .config
        .tui
        .terminal
        .notifications = crate::config::TerminalNotificationMethod::Bell;
    fixture.controller.app_mut().config.tui.terminal.title = false;
    fixture.controller.app_mut().terminal_focused = false;
    let screen = fixture.screen.clone();
    let (release, released) = tokio::sync::oneshot::channel();
    let (finished, finished_rx) = tokio::sync::oneshot::channel();
    let (_events, receiver) = mpsc::unbounded_channel();
    fixture.controller.app_mut().bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Rebuild,
        receiver,
        handle: tokio::spawn(async move {
            released.await.unwrap();
            finished.send(()).unwrap();
            TaskCompletion::Rebuild {
                result: Err(anyhow::anyhow!("scripted rebuild failure")),
            }
        }),
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: 100,
        cancellation_token: None,
        query_control: None,
    });
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        release.send(()).unwrap();
        finished_rx.await.unwrap();
        assert!(poll!(&mut future).is_pending());
        advance(Duration::from_millis(18)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(frame_count(&screen), 2);
        assert!(!screen.borrow().output.contains(&7));
        assert!(
            screen
                .borrow()
                .parser
                .screen()
                .contents()
                .contains("scripted rebuild failure")
        );
    }
    assert!(fixture.controller.app().bottom_pane.running_task.is_none());
}

#[path = "event_loop_goal_tests.rs"]
mod goal_tests;

#[path = "event_loop_exit_tests.rs"]
mod exit_tests;

#[cfg(unix)]
#[path = "event_loop_session_tests.rs"]
mod session_tests;

#[path = "event_loop_feedback_tests.rs"]
mod feedback_tests;

#[tokio::test]
async fn external_editor_drains_runtime_without_painting_until_handoff_finishes() {
    let mut fixture = Fixture::new().await;
    fixture
        .controller
        .app_mut()
        .set_input("original draft".into());
    let input = fixture.input.clone();
    let screen = fixture.screen.clone();
    let commands = fixture.commands.clone();
    let probe = fixture.source.probe.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    *probe.editor_result.borrow_mut() = Some(receiver);
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        input
            .send(Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('g'),
                KeyModifiers::CONTROL,
            ))))
            .unwrap();
        assert!(poll!(&mut future).is_pending());
        assert_eq!(*probe.editor_seeds.borrow(), ["original draft"]);
        let bytes = screen.borrow().output.len();
        let maintenance = probe.maintenance_ticks.get();
        commands
            .send(RuntimeCommand::SetPermissionMode(PermissionMode::ReadOnly))
            .unwrap();
        assert!(poll!(&mut future).is_pending());
        advance(Duration::from_secs(1)).await;
        assert!(poll!(&mut future).is_pending());
        assert_eq!(screen.borrow().output.len(), bytes);
        assert_eq!(probe.maintenance_ticks.get(), maintenance);
        sender.send(Ok("edited draft".into())).unwrap();
        assert!(poll!(&mut future).is_pending());
        advance(Duration::from_millis(18)).await;
        assert!(poll!(&mut future).is_pending());
        let contents = screen.borrow().parser.screen().contents();
        assert!(contents.contains("edited draft"), "{contents}");
        assert!(contents.contains("read-only planning"), "{contents}");
    }
    assert_eq!(fixture.controller.app().bottom_pane.input, "edited draft");
    assert_eq!(
        fixture.controller.app().permission_mode,
        PermissionMode::ReadOnly
    );
    assert!(fixture.port.commands().is_empty());
}
