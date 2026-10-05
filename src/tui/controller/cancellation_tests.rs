use std::sync::Arc;

use futures::StreamExt;

use super::TuiController;
use crate::runtime_control::{
    AssistantEvent, RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, SessionEvent, ToolEvent,
};
use crate::tui::runtime_port::RuntimeProjectionEvent;
use crate::tui::state::{RuntimeSnapshot, TuiApp};
use crate::tui::testing::FakeRuntimeClient;

fn fixture() -> (tempfile::TempDir, TuiController, Arc<FakeRuntimeClient>) {
    let dir = tempfile::tempdir().unwrap();
    let mut app = TuiApp::new(crate::config::ConfigManager {
        path: dir.path().join("config.json"),
    })
    .unwrap();
    app.snapshot.session_id = "session-a".into();
    let port = Arc::new(FakeRuntimeClient::new(RuntimeSnapshot::default()));
    let (_, commands) = tokio::sync::mpsc::unbounded_channel();
    let controller = TuiController::new(app, port.clone(), commands);
    (dir, controller, port)
}

fn event(session: &str, turn: &str, sequence: u64, event: RuntimeEvent) -> RuntimeProjectionEvent {
    RuntimeProjectionEvent::Runtime(Box::new(RuntimeControlEvent {
        event_id: format!("event-{sequence}"),
        provenance: RuntimeProvenance::local_tui(session),
        turn_id: Some(turn.into()),
        sequence,
        event,
    }))
}

async fn deliver(
    controller: &mut TuiController,
    port: &FakeRuntimeClient,
    event: RuntimeProjectionEvent,
) -> bool {
    port.emit(event);
    let event = controller
        .runtime_events
        .next()
        .await
        .expect("scripted runtime event");
    controller.apply_runtime_event(event)
}

#[tokio::test]
async fn terminal_turn_drops_late_text_and_tool_events() {
    let (_dir, mut controller, port) = fixture();
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                1,
                RuntimeEvent::Session(SessionEvent::TurnStarted)
            )
        )
        .await
    );
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                2,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta("Partial answer".into()))
            )
        )
        .await
    );
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                3,
                RuntimeEvent::Session(SessionEvent::TurnCancelled)
            )
        )
        .await
    );
    controller.app.finalize_agent_stream(None);
    controller.app.finalize_active_turn();
    let committed = controller.app.committed_turns.len();
    controller.needs_redraw = false;
    for (sequence, tail) in [
        (
            4,
            RuntimeEvent::Assistant(AssistantEvent::TextDelta("orphan".into())),
        ),
        (
            5,
            RuntimeEvent::Tool(ToolEvent::Use {
                call_id: Some("late-tool".into()),
                name: "bash".into(),
                input: serde_json::json!({"command":"true"}),
            }),
        ),
    ] {
        assert!(
            !deliver(
                &mut controller,
                &port,
                event("session-a", "turn-a", sequence, tail)
            )
            .await
        );
    }
    assert!(!controller.app.has_agent_stream());
    assert!(controller.app.active_turn.entries.is_empty());
    assert_eq!(controller.app.committed_turns.len(), committed);
    assert!(!controller.needs_redraw);
}

#[tokio::test]
async fn foreign_session_cannot_replace_the_sequence_cursor() {
    let (_dir, mut controller, port) = fixture();
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                1,
                RuntimeEvent::Session(SessionEvent::TurnStarted)
            )
        )
        .await
    );
    assert!(
        !deliver(
            &mut controller,
            &port,
            event(
                "session-b",
                "turn-b",
                100,
                RuntimeEvent::Session(SessionEvent::TurnCancelled)
            )
        )
        .await
    );
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                2,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta("Valid".into()))
            )
        )
        .await
    );
    assert_eq!(controller.last_runtime_event.as_ref().unwrap().1, 2);
    assert!(
        !deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                1,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta("duplicate".into()))
            )
        )
        .await
    );
}

#[tokio::test]
async fn new_turn_rejects_previous_turn_terminal_and_tail() {
    let (_dir, mut controller, port) = fixture();
    for (turn, sequence, state) in [
        ("turn-a", 1, SessionEvent::TurnStarted),
        ("turn-a", 2, SessionEvent::TurnFinished { reason: None }),
        ("turn-b", 3, SessionEvent::TurnStarted),
    ] {
        assert!(
            deliver(
                &mut controller,
                &port,
                event("session-a", turn, sequence, RuntimeEvent::Session(state))
            )
            .await
        );
    }
    assert!(
        !deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                4,
                RuntimeEvent::Session(SessionEvent::TurnCancelled)
            )
        )
        .await
    );
    assert!(
        !deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                5,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta("stale".into()))
            )
        )
        .await
    );
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-b",
                4,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta("Current".into()))
            )
        )
        .await
    );
    assert_eq!(
        controller
            .app
            .agent_markdown_stream
            .as_ref()
            .unwrap()
            .raw_text,
        "Current"
    );
}

#[tokio::test]
async fn scoped_completion_barrier_ignores_wrong_identity_and_unscoped_completion() {
    use crate::runtime_control::ErrorEvent;
    use crate::tui::runtime::QueryTaskControl;
    use crate::tui::state::{RunningTask, TaskCompletion, TaskKind};

    let (_dir, mut controller, port) = fixture();
    let control = QueryTaskControl::new("session-a".into());
    let (_, receiver) = tokio::sync::mpsc::unbounded_channel();
    controller.app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(std::future::pending::<TaskCompletion>()),
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: Some(control.clone()),
    });
    controller
        .query_completion_barrier
        .defer(super::tests::test_completion());
    for (session, turn) in [
        ("session-b", control.turn_id.as_str()),
        ("session-a", "other-turn"),
    ] {
        assert!(
            !deliver(
                &mut controller,
                &port,
                event(
                    session,
                    turn,
                    100,
                    RuntimeEvent::Session(SessionEvent::TurnCancelled)
                )
            )
            .await
        );
        assert!(controller.query_completion_barrier.take_ready().is_none());
    }
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                &control.turn_id,
                1,
                RuntimeEvent::Error(ErrorEvent::RuntimeError {
                    message: "diagnostic".into(),
                    recoverable: false
                })
            )
        )
        .await
    );
    assert!(controller.query_completion_barrier.take_ready().is_none());
    assert!(!controller.apply_runtime_event(RuntimeProjectionEvent::Completed { reason: None }));
    assert!(controller.query_completion_barrier.take_ready().is_none());
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                &control.turn_id,
                2,
                RuntimeEvent::Session(SessionEvent::TurnCancelled)
            )
        )
        .await
    );
    assert!(controller.query_completion_barrier.take_ready().is_some());
    assert!(controller.query_completion_barrier.take_ready().is_none());
    controller
        .app
        .bottom_pane
        .running_task
        .take()
        .unwrap()
        .handle
        .abort();
}

#[tokio::test]
async fn unscoped_catalog_event_cannot_reset_the_session_sequence_watermark() {
    let (_dir, mut controller, port) = fixture();
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                1,
                RuntimeEvent::Session(SessionEvent::TurnStarted)
            )
        )
        .await
    );
    let mut catalog = match event(
        "session-a",
        "turn-a",
        3,
        RuntimeEvent::Input(crate::runtime_control::InputEvent::UserPromptSubmitted),
    ) {
        RuntimeProjectionEvent::Runtime(event) => event,
        _ => unreachable!(),
    };
    catalog.provenance.session_id = None;
    catalog.turn_id = None;
    assert!(
        deliver(
            &mut controller,
            &port,
            RuntimeProjectionEvent::Runtime(catalog.clone())
        )
        .await
    );
    catalog.sequence = 0;
    catalog.event_id = "unsequenced".into();
    assert!(
        deliver(
            &mut controller,
            &port,
            RuntimeProjectionEvent::Runtime(catalog)
        )
        .await
    );
    assert!(
        !deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                2,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta("duplicate".into()))
            )
        )
        .await
    );
    assert!(
        deliver(
            &mut controller,
            &port,
            event(
                "session-a",
                "turn-a",
                4,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta("Current".into()))
            )
        )
        .await
    );
}
