use futures::StreamExt;

use super::*;
use crate::runtime_client::{RuntimeClient, RuntimeTaskServices};
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};
use crate::runtime_control::{
    AssistantEvent, InputControlRequest, RuntimeControlEvent, SessionControlRequest,
};
use crate::tui::controller::TuiController;
use crate::tui::input_control::{InputControlOutcome, handle_session_control};
use crate::tui::runtime::RuntimeCommandProcessor;
use crate::tui::runtime_port::{
    RuntimeClientPort, RuntimeCommand, RuntimeEventStream, RuntimeProjectionEvent,
};
use crate::tui::state::RuntimeSnapshot;
use crate::tui::testing::FakeRuntimeClient;

#[derive(Clone, Copy)]
enum BackendOutcome {
    Answer,
    Failure,
    Panic,
}

struct DrainingBackend {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    observed_stop: AtomicBool,
    outcome: BackendOutcome,
}

impl DrainingBackend {
    fn new(outcome: BackendOutcome) -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            observed_stop: AtomicBool::new(false),
            outcome,
        }
    }
}

#[async_trait::async_trait]
impl LlmBackend for DrainingBackend {
    async fn ask(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        match self.outcome {
            BackendOutcome::Answer => Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "Before. Tail.".into(),
                }],
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
            BackendOutcome::Failure => Err(anyhow::anyhow!("scripted provider failure")),
            BackendOutcome::Panic => panic!("scripted provider panic"),
        }
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        metadata: crate::llm::LlmTurnMetadata,
        on_event: &mut (dyn FnMut(crate::llm::LlmStreamEvent) + Send),
    ) -> anyhow::Result<LlmResponse> {
        on_event(crate::llm::LlmStreamEvent::TextDelta("Before. ".into()));
        self.entered.notify_one();
        self.release.notified().await;
        self.observed_stop
            .store(metadata.is_cancelled(), Ordering::SeqCst);
        on_event(crate::llm::LlmStreamEvent::TextDelta("Tail.".into()));
        self.ask(messages, tools).await
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        Ok("summary".into())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    controller: TuiController,
    processor: RuntimeCommandProcessor,
    port: Arc<FakeRuntimeClient>,
    projections: RuntimeEventStream,
    events: tokio::sync::broadcast::Receiver<RuntimeControlEvent>,
    backend: Arc<DrainingBackend>,
    _commands: mpsc::UnboundedSender<RuntimeCommand>,
}

impl Fixture {
    async fn start(outcome: BackendOutcome) -> Self {
        let dir = tempdir().unwrap();
        let backend = Arc::new(DrainingBackend::new(outcome));
        let mut config = RaraConfig::default();
        config.builtin_plugins.nowledge_mem.enabled = false;
        let options = RuntimeBootstrapOptions::with_plugin_dirs(Vec::new())
            .with_rara_home(Some(dir.path().join("state")))
            .with_backend(Some(backend.clone()))
            .with_tool_manager(Some(ToolManager::new()))
            .with_extension_discovery(false)
            .with_memory_facilities(false)
            .with_transcript_persistence(false);
        let bootstrap = initialize_rara_context_for_workspace_with_options(
            &config,
            Some(dir.path()),
            None,
            options,
        )
        .await
        .unwrap();
        let runtime = RuntimeClient::from_bootstrap(bootstrap).await;
        let events = runtime.event_bus.subscribe_control();
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .unwrap();
        app.config = config;
        app.event_bus = Some(runtime.event_bus.clone());
        app.mcp_manager = Some(runtime.mcp_manager.clone());
        app.memory_handler = Some(Arc::new(
            crate::protocol_sources::MemoryControlHandler::new(runtime.event_bus.clone()),
        ));
        let RuntimeTaskServices {
            prompt_source_registry,
            skill_source_registry,
            hook_registry,
        } = runtime.task_services();
        app.prompt_source_registry = Some(prompt_source_registry);
        app.skill_source_registry = Some(skill_source_registry);
        app.hook_registry = Some(hook_registry);
        let mut processor = RuntimeCommandProcessor::new(runtime);
        processor.sync_snapshot(&mut app);
        let port = Arc::new(FakeRuntimeClient::new(RuntimeSnapshot::default()));
        let projections = port.subscribe();
        let (commands, receiver) = mpsc::unbounded_channel();
        let mut controller = TuiController::new(app, port.clone(), receiver);
        controller
            .apply_runtime_command(
                &mut processor,
                RuntimeCommand::Input(InputControlRequest::SubmitUserPrompt {
                    prompt: "Inspect the cancellation boundary.".into(),
                }),
            )
            .await
            .unwrap();
        backend.entered.notified().await;
        Self {
            _dir: dir,
            controller,
            processor,
            port,
            projections,
            events,
            backend,
            _commands: commands,
        }
    }

    fn drain_events(&mut self) -> Vec<RuntimeControlEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }

    async fn deliver(&mut self, event: RuntimeControlEvent) -> bool {
        self.port
            .emit(RuntimeProjectionEvent::Runtime(Box::new(event)));
        let event = self.projections.next().await.expect("scripted event");
        self.controller.apply_runtime_event(event)
    }

    async fn task_return(&mut self) -> Box<Result<TaskCompletion, tokio::task::JoinError>> {
        Box::new(
            (&mut self
                .controller
                .app_mut()
                .bottom_pane
                .running_task
                .as_mut()
                .unwrap()
                .handle)
                .await,
        )
    }
}

#[tokio::test]
async fn query_stop_drains_tail_before_terminal_in_both_completion_orders() {
    for (request, detail) in [
        (SessionControlRequest::CancelCurrentTurn, "query cancelled"),
        (
            SessionControlRequest::InterruptCurrentTurn,
            "query interrupted",
        ),
    ] {
        for completion_first in [false, true] {
            let mut fixture = Fixture::start(BackendOutcome::Answer).await;
            let control = fixture
                .controller
                .app()
                .bottom_pane
                .running_task
                .as_ref()
                .unwrap()
                .query_control
                .clone()
                .unwrap();
            for event in fixture.drain_events() {
                assert!(fixture.deliver(event).await);
            }
            fixture
                .controller
                .apply_runtime_command(
                    &mut fixture.processor,
                    RuntimeCommand::Session(request.clone()),
                )
                .await
                .unwrap();
            assert!(control.stop_kind().is_some());
            assert!(
                fixture.drain_events().is_empty(),
                "request must not publish terminal"
            );
            assert_eq!(
                handle_session_control(
                    fixture.controller.app_mut(),
                    SessionControlRequest::CancelCurrentTurn
                ),
                InputControlOutcome::Rejected
            );
            fixture.backend.release.notify_one();
            let completion = fixture.task_return().await;
            assert!(fixture.backend.observed_stop.load(Ordering::SeqCst));
            let events = fixture.drain_events();
            let terminal_index = events
                .iter()
                .position(|event| {
                    matches!(
                        event.event,
                        RuntimeEvent::Session(
                            SessionEvent::TurnCancelled | SessionEvent::TurnInterrupted
                        )
                    )
                })
                .unwrap();
            assert_eq!(terminal_index, events.len() - 1);
            assert!(events[..terminal_index].iter().any(|event| matches!(&event.event, RuntimeEvent::Assistant(AssistantEvent::TextDelta(text)) if text == "Tail.")));
            assert!(events.iter().all(|event| event.turn_id.as_deref()
                == Some(control.turn_id.as_str())
                && event.provenance.session_id.as_deref() == Some(control.session_id.as_str())));
            assert!(
                events
                    .windows(2)
                    .all(|pair| pair[0].sequence < pair[1].sequence)
            );
            let terminal = events.last().unwrap().clone();
            match request {
                SessionControlRequest::CancelCurrentTurn => assert_eq!(
                    terminal.event,
                    RuntimeEvent::Session(SessionEvent::TurnCancelled)
                ),
                SessionControlRequest::InterruptCurrentTurn => assert_eq!(
                    terminal.event,
                    RuntimeEvent::Session(SessionEvent::TurnInterrupted)
                ),
                _ => unreachable!(),
            }
            if completion_first {
                assert!(
                    !fixture
                        .controller
                        .receive_runtime_task_completion(&mut fixture.processor, completion)
                        .await
                        .unwrap()
                );
                for event in events {
                    assert!(fixture.deliver(event).await);
                }
                assert!(
                    fixture
                        .controller
                        .complete_query_if_ready(&mut fixture.processor)
                        .await
                        .unwrap()
                );
            } else {
                for event in events {
                    assert!(fixture.deliver(event).await);
                }
                assert!(
                    !fixture
                        .controller
                        .complete_query_if_ready(&mut fixture.processor)
                        .await
                        .unwrap()
                );
                assert!(
                    fixture
                        .controller
                        .receive_runtime_task_completion(&mut fixture.processor, completion)
                        .await
                        .unwrap()
                );
            }
            assert!(fixture.controller.app().bottom_pane.running_task.is_none());
            assert!(!fixture.controller.app().has_agent_stream());
            assert!(fixture.controller.app().active_turn.entries.is_empty());
            assert_eq!(fixture.controller.app().runtime_phase, RuntimePhase::Idle);
            assert_eq!(
                fixture.controller.app().runtime_phase_detail.as_deref(),
                Some(detail)
            );
            let committed: String = fixture
                .controller
                .app()
                .committed_turns
                .iter()
                .flat_map(|turn| turn.entries.iter())
                .map(|entry| entry.message.as_str())
                .collect();
            assert!(committed.contains("Before. Tail."), "{committed}");
            let mut late = terminal;
            late.sequence += 1;
            late.event_id = "late-event".into();
            late.event = RuntimeEvent::Assistant(AssistantEvent::TextDelta("orphan".into()));
            assert!(!fixture.deliver(late).await);
            assert!(!fixture.controller.app().has_agent_stream());
        }
    }
}

#[tokio::test]
async fn task_return_rejects_cancel_before_ui_observes_completion() {
    let mut fixture = Fixture::start(BackendOutcome::Answer).await;
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    fixture.controller.app_mut().bottom_pane.notice = Some("Cancellation requested.".into());
    assert_eq!(
        handle_session_control(
            fixture.controller.app_mut(),
            SessionControlRequest::CancelCurrentTurn
        ),
        InputControlOutcome::Rejected
    );
    assert!(!fixture.backend.observed_stop.load(Ordering::SeqCst));
    let events = fixture.drain_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeEvent::Session(SessionEvent::TurnFinished { .. })
            ))
            .count(),
        1
    );
    assert!(!events.iter().any(|event| matches!(
        event.event,
        RuntimeEvent::Session(SessionEvent::TurnCancelled)
    )));
    assert!(
        !fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion)
            .await
            .unwrap()
    );
    for event in events {
        assert!(fixture.deliver(event).await);
    }
    assert!(
        fixture
            .controller
            .complete_query_if_ready(&mut fixture.processor)
            .await
            .unwrap()
    );
    assert_eq!(
        fixture.controller.app().runtime_phase_detail.as_deref(),
        Some("prompt finished")
    );
}

#[tokio::test]
async fn query_failure_publishes_diagnostic_before_one_terminal_boundary() {
    let mut fixture = Fixture::start(BackendOutcome::Failure).await;
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    let events = fixture.drain_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeEvent::Session(SessionEvent::TurnFailed { .. })
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeEvent::Error(ErrorEvent::RuntimeError {
                    recoverable: false,
                    ..
                })
            ))
            .count(),
        1
    );
    assert!(
        !fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion)
            .await
            .unwrap()
    );
    let final_index = events.len() - 1;
    for (index, event) in events.into_iter().enumerate() {
        assert!(fixture.deliver(event).await);
        assert_eq!(
            fixture
                .controller
                .complete_query_if_ready(&mut fixture.processor)
                .await
                .unwrap(),
            index == final_index
        );
    }
    assert_eq!(fixture.controller.app().runtime_phase, RuntimePhase::Failed);
}

#[tokio::test]
async fn query_join_failure_closes_identity_without_waiting_for_terminal() {
    let mut fixture = Fixture::start(BackendOutcome::Panic).await;
    let control = fixture
        .controller
        .app()
        .bottom_pane
        .running_task
        .as_ref()
        .unwrap()
        .query_control
        .clone()
        .unwrap();
    // No projection has been consumed yet: closing must establish the fence
    // even when execution panics before the UI observes TurnStarted.
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    assert!(completion.is_err());
    assert_eq!(
        handle_session_control(
            fixture.controller.app_mut(),
            SessionControlRequest::CancelCurrentTurn
        ),
        InputControlOutcome::Rejected
    );
    assert!(
        fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion)
            .await
            .is_err()
    );
    assert!(fixture.controller.app().bottom_pane.running_task.is_none());
    for event in fixture.drain_events() {
        let belongs_to_query = event.turn_id.as_deref() == Some(control.turn_id.as_str());
        let applied = fixture.deliver(event.clone()).await;
        if belongs_to_query {
            assert!(!applied, "terminal query event was accepted: {event:?}");
        }
    }
    let late = RuntimeControlEvent {
        event_id: "late-after-panic".into(),
        provenance: RuntimeProvenance::local_tui(control.session_id),
        turn_id: Some(control.turn_id),
        sequence: 100,
        event: RuntimeEvent::Assistant(AssistantEvent::TextDelta("orphan".into())),
    };
    assert!(!fixture.deliver(late).await);
    assert!(!fixture.controller.app().has_agent_stream());
}

#[tokio::test]
async fn cancelled_turn_tail_cannot_finish_or_modify_queued_query() {
    let mut fixture = Fixture::start(BackendOutcome::Answer).await;
    let previous = fixture
        .controller
        .app()
        .bottom_pane
        .running_task
        .as_ref()
        .unwrap()
        .query_control
        .clone()
        .unwrap();
    for event in fixture.drain_events() {
        assert!(fixture.deliver(event).await);
    }
    fixture
        .controller
        .apply_runtime_command(
            &mut fixture.processor,
            RuntimeCommand::Input(InputControlRequest::SubmitUserPrompt {
                prompt: "Follow up after cancellation.".into(),
            }),
        )
        .await
        .unwrap();
    fixture
        .controller
        .apply_runtime_command(
            &mut fixture.processor,
            RuntimeCommand::Session(SessionControlRequest::CancelCurrentTurn),
        )
        .await
        .unwrap();
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    assert!(
        !fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion)
            .await
            .unwrap()
    );
    for event in fixture.drain_events() {
        assert!(fixture.deliver(event).await);
    }
    assert!(
        fixture
            .controller
            .complete_query_if_ready(&mut fixture.processor)
            .await
            .unwrap()
    );
    let current = fixture
        .controller
        .app()
        .bottom_pane
        .running_task
        .as_ref()
        .unwrap()
        .query_control
        .clone()
        .unwrap();
    assert_ne!(current.turn_id, previous.turn_id);
    for event in [
        RuntimeEvent::Assistant(AssistantEvent::TextDelta("orphan".into())),
        RuntimeEvent::Session(SessionEvent::TurnCancelled),
    ] {
        assert!(
            !fixture
                .deliver(RuntimeControlEvent {
                    event_id: "stale-turn-event".into(),
                    provenance: RuntimeProvenance::local_tui(previous.session_id.clone()),
                    turn_id: Some(previous.turn_id.clone()),
                    sequence: 1000,
                    event,
                })
                .await
        );
    }
    assert!(
        !fixture
            .controller
            .complete_query_if_ready(&mut fixture.processor)
            .await
            .unwrap()
    );
    fixture.backend.entered.notified().await;
    for event in fixture.drain_events() {
        assert!(fixture.deliver(event).await);
    }
    assert_eq!(
        fixture
            .controller
            .app()
            .agent_markdown_stream
            .as_ref()
            .unwrap()
            .raw_text,
        "Before. "
    );
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    assert!(
        !fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion)
            .await
            .unwrap()
    );
    for event in fixture.drain_events() {
        assert!(fixture.deliver(event).await);
    }
    assert!(
        fixture
            .controller
            .complete_query_if_ready(&mut fixture.processor)
            .await
            .unwrap()
    );
    assert!(fixture.controller.app().bottom_pane.running_task.is_none());
    assert_eq!(
        fixture.controller.app().runtime_phase_detail.as_deref(),
        Some("prompt finished")
    );
}
