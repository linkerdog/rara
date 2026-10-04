use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use rara_tools::tool::ToolManager;

use super::*;
use crate::agent::AgentEvent;
use crate::config::{ConfigManager, RaraConfig};
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, Message, MockLlm};
use crate::runtime_client::{RuntimeClient, RuntimeTaskServices};
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};
use crate::runtime_control::{
    InputControlRequest, PlanEvent, RuntimeEvent, RuntimeProvenance, WarningEvent,
};
use crate::runtime_event_bus::{RuntimeEventBus, RuntimeEventCapacity};
use crate::tui::runtime_port::InProcessRuntimeClientPort;

struct Fixture {
    _dir: tempfile::TempDir,
    controller: TuiController,
    processor: RuntimeCommandProcessor,
    bus: Arc<RuntimeEventBus>,
}

impl Fixture {
    async fn new(capacity: RuntimeEventCapacity, backend: Arc<dyn LlmBackend>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = RaraConfig::default();
        config.builtin_plugins.nowledge_mem.enabled = false;
        let mut options = RuntimeBootstrapOptions::with_plugin_dirs(Vec::new())
            .with_rara_home(Some(dir.path().join("state")))
            .with_backend(Some(backend))
            .with_tool_manager(Some(ToolManager::new()))
            .with_extension_discovery(false)
            .with_memory_facilities(false)
            .with_transcript_persistence(false)
            .with_event_capacity(capacity.broadcast);
        options.replay_capacity = capacity.replay;
        let bootstrap = initialize_rara_context_for_workspace_with_options(
            &config,
            Some(dir.path()),
            None,
            options,
        )
        .await
        .unwrap();
        let runtime = RuntimeClient::from_bootstrap(bootstrap).await;
        let bus = runtime.event_bus.clone();
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .unwrap();
        app.config = config;
        app.event_bus = Some(bus.clone());
        app.mcp_manager = Some(runtime.mcp_manager.clone());
        app.goal_handle = runtime.goal_handle.clone();
        app.memory_handler = Some(Arc::new(
            crate::protocol_sources::MemoryControlHandler::new(bus.clone()),
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
        let (port, commands) = InProcessRuntimeClientPort::new(
            bus.clone(),
            Arc::new(RwLock::new(app.snapshot.clone().into_inner())),
        );
        Self {
            _dir: dir,
            controller: TuiController::new(app, Arc::new(port), commands),
            processor,
            bus,
        }
    }

    fn publish(&self, event: RuntimeEvent) {
        self.bus.publish_control_with_turn(
            event,
            RuntimeProvenance::local_tui(self.controller.app.snapshot.session_id.clone()),
            None,
        );
    }

    async fn pump_available(&mut self) {
        while let Some(Some(event)) = self.controller.runtime_events.next().now_or_never() {
            self.controller.apply_runtime_event(event);
            self.controller.resync_after_event_loss(&mut self.processor);
        }
    }

    fn messages(&self) -> Vec<String> {
        self.controller
            .app
            .committed_turns
            .iter()
            .flat_map(|turn| &turn.entries)
            .chain(self.controller.app.active_turn.entries.iter())
            .map(|entry| entry.message.clone())
            .collect()
    }
}

#[tokio::test]
async fn lagged_non_query_projection_matches_uninterrupted_projection() {
    let mut fast = Fixture::new(
        RuntimeEventCapacity {
            broadcast: 2,
            replay: 16,
        },
        Arc::new(MockLlm),
    )
    .await;
    let mut slow = Fixture::new(
        RuntimeEventCapacity {
            broadcast: 2,
            replay: 16,
        },
        Arc::new(MockLlm),
    )
    .await;
    // Establish live receivers before producing the burst.
    fast.pump_available().await;
    slow.pump_available().await;
    for n in 0..6 {
        for event in [
            RuntimeEvent::Plan(PlanEvent::Updated {
                steps: Vec::new(),
                explanation: Some(format!("plan {n}")),
            }),
            RuntimeEvent::Warning(WarningEvent::RuntimeWarning {
                message: format!("background warning {n}"),
            }),
        ] {
            fast.publish(event.clone());
            fast.pump_available().await;
            slow.publish(event);
        }
    }
    slow.pump_available().await;
    assert_eq!(slow.messages(), fast.messages());
    assert_eq!(
        slow.controller.app.snapshot.plan_explanation,
        fast.controller.app.snapshot.plan_explanation
    );
    assert!(slow.controller.runtime_resync_through.is_none());
    assert!(
        !slow
            .messages()
            .iter()
            .any(|message| message.contains("could not be recovered"))
    );
}

#[tokio::test]
async fn exhausted_window_refreshes_owned_state_and_defers_busy_agent() {
    let mut fixture = Fixture::new(
        RuntimeEventCapacity {
            broadcast: 2,
            replay: 2,
        },
        Arc::new(MockLlm),
    )
    .await;
    fixture.pump_available().await;
    let mut agent = fixture.processor.agent_mut().take().unwrap();
    agent.history.push(Message {
        role: "user".into(),
        content: serde_json::json!("new authoritative history"),
    });
    agent.pending_user_input = Some(crate::agent::PendingUserInput {
        question: "Current approval question".into(),
        options: Vec::new(),
        note: None,
    });
    fixture
        .controller
        .app
        .cache_running_action("stale tool progress");
    fixture.controller.app.snapshot.history_len = 999;
    fixture.controller.publish_snapshot_projection();
    let goal = crate::runtime_goals::RalphGoal::new("fresh goal".into(), None);
    fixture
        .controller
        .app
        .goal_handle
        .replace(Some(goal.clone()))
        .unwrap();
    for n in 0..8 {
        fixture.bus.send_with_provenance(
            AgentEvent::Status(n.to_string()),
            RuntimeProvenance::local_tui(agent.session_id.clone()),
        );
    }
    fixture.pump_available().await;
    assert!(fixture.controller.runtime_resync_through.is_some());
    assert_eq!(fixture.controller.app.goal, Some(goal));
    // The agent is still owned by execution, so another overflow belongs to
    // the same pending refresh and must not append a second loss notice.
    for n in 0..8 {
        fixture.bus.send_with_provenance(
            AgentEvent::Status(format!("second burst {n}")),
            RuntimeProvenance::local_tui(agent.session_id.clone()),
        );
    }
    fixture.pump_available().await;
    assert_eq!(
        fixture
            .messages()
            .iter()
            .filter(|message| message.contains("could not be recovered"))
            .count(),
        1
    );
    let expected_history_len = agent.history.len();
    *fixture.processor.agent_mut() = Some(agent);
    assert!(
        fixture
            .controller
            .resync_after_event_loss(&mut fixture.processor)
    );
    assert!(fixture.controller.runtime_resync_through.is_none());
    assert!(
        fixture
            .controller
            .app
            .active_live
            .running_actions
            .is_empty()
    );
    assert_eq!(
        fixture.controller.app.runtime_phase,
        crate::tui::state::RuntimePhase::Idle
    );
    assert_eq!(
        fixture.controller.app.snapshot.history_len,
        expected_history_len
    );
    assert_eq!(
        fixture
            .controller
            .app
            .pending_request_input()
            .unwrap()
            .title,
        "Current approval question"
    );
    assert_eq!(
        fixture
            .controller
            .runtime_port
            .snapshot()
            .await
            .unwrap()
            .history_len,
        expected_history_len
    );
    assert!(
        !fixture
            .controller
            .resync_after_event_loss(&mut fixture.processor)
    );
}

#[tokio::test]
async fn completion_respects_stream_boundary_with_later_query_receipts_pending() {
    use crate::tui::runtime::QueryTaskControl;
    use crate::tui::state::{RunningTask, TaskKind};

    let mut fixture = Fixture::new(
        RuntimeEventCapacity {
            broadcast: 8,
            replay: 8,
        },
        Arc::new(MockLlm),
    )
    .await;
    let initial_cursor = fixture.controller.runtime_cursor;
    let agent = fixture.processor.agent_mut().take().unwrap();
    let control = QueryTaskControl::new(agent.session_id.clone());
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    fixture.controller.app.begin_running_turn();
    fixture.controller.app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(async move {
            TaskCompletion::Query {
                agent,
                result: Ok(()),
                goal_turn: None,
            }
        }),
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: Some(control.clone()),
    });
    for label in [
        "boundary query one",
        "boundary background one",
        "boundary query two",
        "boundary background two",
    ] {
        let event = RuntimeEvent::Warning(WarningEvent::RuntimeWarning {
            message: label.into(),
        });
        if label.contains("query") {
            control.publish_event(
                &fixture.bus,
                &sender,
                RuntimeControlEvent {
                    event_id: String::new(),
                    sequence: 0,
                    turn_id: None,
                    provenance: RuntimeProvenance::local_tui(control.session_id.clone()),
                    event,
                },
            );
        } else {
            fixture.publish(event);
        }
    }
    control
        .publish_finished(&fixture.bus, &sender, Ok(()))
        .unwrap();
    let published = fixture.bus.current_sequence();
    let first = fixture.controller.runtime_events.next().await.unwrap();
    fixture.controller.apply_runtime_event(first);
    assert_eq!(fixture.controller.runtime_cursor, initial_cursor + 1);
    assert_eq!(
        fixture
            .controller
            .pending_query_receipt
            .as_ref()
            .unwrap()
            .sequence,
        initial_cursor + 3,
        "a future receipt must stay parked behind the stream boundary"
    );
    let completion = (&mut fixture
        .controller
        .app
        .bottom_pane
        .running_task
        .as_mut()
        .unwrap()
        .handle)
        .await;
    fixture
        .controller
        .receive_runtime_task_completion(&mut fixture.processor, Box::new(completion))
        .await
        .unwrap();
    assert_eq!(fixture.controller.runtime_cursor, published);
    let messages = fixture.messages();
    let boundary_messages = messages
        .iter()
        .filter(|text| text.starts_with("boundary "))
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        boundary_messages,
        [
            "boundary query one",
            "boundary background one",
            "boundary query two",
            "boundary background two"
        ]
    );
    assert!(!fixture.controller.app.is_busy());
    assert!(fixture.processor.agent().is_some());
}

#[derive(Default)]
struct WaitingBackend {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[tokio::test]
async fn snapshot_refresh_follows_the_retained_tail() {
    let mut fixture = Fixture::new(
        RuntimeEventCapacity {
            broadcast: 2,
            replay: 2,
        },
        Arc::new(MockLlm),
    )
    .await;
    fixture
        .processor
        .agent_mut()
        .as_mut()
        .unwrap()
        .plan_explanation = Some("current authoritative plan".into());
    for n in 0..4 {
        fixture.bus.send_with_provenance(
            AgentEvent::Status(n.to_string()),
            RuntimeProvenance::runtime(None),
        );
    }
    fixture.publish(RuntimeEvent::Plan(PlanEvent::Updated {
        steps: Vec::new(),
        explanation: Some("older retained plan".into()),
    }));
    fixture.bus.send_with_provenance(
        AgentEvent::Status("tail".into()),
        RuntimeProvenance::runtime(None),
    );
    fixture.pump_available().await;
    assert_eq!(
        fixture.controller.app.snapshot.plan_explanation.as_deref(),
        Some("current authoritative plan")
    );
    assert!(fixture.controller.runtime_resync_through.is_none());
    assert_eq!(
        fixture.controller.app.runtime_phase,
        crate::tui::state::RuntimePhase::Idle
    );
}

#[async_trait::async_trait]
impl LlmBackend for WaitingBackend {
    async fn ask(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Recovered answer".into(),
            }],
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }
    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        Ok("summary".into())
    }
}

#[tokio::test]
async fn completion_replay_preserves_interleaved_background_events_once() {
    let backend = Arc::new(WaitingBackend::default());
    let mut fixture = Fixture::new(
        RuntimeEventCapacity {
            broadcast: 2,
            replay: 64,
        },
        backend.clone(),
    )
    .await;
    fixture
        .controller
        .apply_runtime_command(
            &mut fixture.processor,
            RuntimeCommand::Input(InputControlRequest::SubmitUserPrompt {
                prompt: "test replay".into(),
            }),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), backend.entered.notified())
        .await
        .unwrap();
    fixture.publish(RuntimeEvent::Warning(WarningEvent::RuntimeWarning {
        message: "interleaved background warning".into(),
    }));
    backend.release.notify_one();
    let completion = tokio::time::timeout(
        Duration::from_secs(5),
        &mut fixture
            .controller
            .app
            .bottom_pane
            .running_task
            .as_mut()
            .unwrap()
            .handle,
    )
    .await
    .unwrap();
    assert!(
        fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, Box::new(completion))
            .await
            .unwrap()
    );
    fixture.pump_available().await;
    assert_eq!(
        fixture
            .messages()
            .iter()
            .filter(|message| message.contains("interleaved background warning"))
            .count(),
        1
    );
    assert_eq!(
        fixture
            .messages()
            .iter()
            .filter(|message| message.contains("Recovered answer"))
            .count(),
        1
    );
    assert!(
        !fixture
            .messages()
            .iter()
            .any(|message| message.contains("could not be recovered"))
    );
    assert!(!fixture.controller.app.is_busy());
}

#[tokio::test]
async fn query_receipts_fill_an_exhausted_replay_window_without_loss_notice() {
    for capacity in [2, 3] {
        let backend = Arc::new(WaitingBackend::default());
        let mut fixture = Fixture::new(
            RuntimeEventCapacity {
                broadcast: capacity,
                replay: capacity,
            },
            backend.clone(),
        )
        .await;
        fixture
            .controller
            .apply_runtime_command(
                &mut fixture.processor,
                RuntimeCommand::Input(InputControlRequest::SubmitUserPrompt {
                    prompt: "test receipt fallback".into(),
                }),
            )
            .await
            .unwrap();
        // Input admission is a separate event, not a query receipt.
        fixture.pump_available().await;
        tokio::time::timeout(Duration::from_secs(5), backend.entered.notified())
            .await
            .unwrap();
        fixture.pump_available().await;
        backend.release.notify_one();
        let completion = tokio::time::timeout(
            Duration::from_secs(5),
            &mut fixture
                .controller
                .app
                .bottom_pane
                .running_task
                .as_mut()
                .unwrap()
                .handle,
        )
        .await
        .unwrap();
        if capacity == 3 {
            // These four live records outlast the three-event replay window.
            fixture.publish(RuntimeEvent::Warning(WarningEvent::RuntimeWarning {
                message: "background beyond replay retention".into(),
            }));
            for n in 0..3 {
                fixture.bus.send_with_provenance(
                    AgentEvent::Status(n.to_string()),
                    RuntimeProvenance::runtime(None),
                );
            }
        }
        assert!(
            fixture
                .bus
                .replay_after(fixture.controller.runtime_cursor)
                .is_err(),
            "the replay window must actually overflow"
        );
        assert!(
            fixture
                .controller
                .receive_runtime_task_completion(&mut fixture.processor, Box::new(completion))
                .await
                .unwrap()
        );
        fixture.pump_available().await;
        assert!(
            !fixture
                .messages()
                .iter()
                .any(|message| message.contains("could not be recovered"))
        );
        assert_eq!(
            fixture
                .messages()
                .iter()
                .filter(|message| message.contains("Recovered answer"))
                .count(),
            1
        );
        assert_eq!(
            fixture
                .messages()
                .iter()
                .filter(|message| message.contains("background beyond replay retention"))
                .count(),
            usize::from(capacity == 3),
        );
        assert!(!fixture.controller.app.is_busy());
    }
}
