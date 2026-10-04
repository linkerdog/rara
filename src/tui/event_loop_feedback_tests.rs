use super::*;
use crate::config::TerminalNotificationMethod;
use crate::runtime_control::ApprovalEvent;
use crate::tui::runtime::QueryTaskControl;

fn count_bytes(screen: &Rc<RefCell<EmulatedScreen>>, sequence: &[u8]) -> usize {
    screen
        .borrow()
        .output
        .windows(sequence.len())
        .filter(|part| *part == sequence)
        .count()
}

#[tokio::test]
async fn completion_bell_waits_for_both_boundaries_in_either_order() {
    for terminal_first in [false, true] {
        let mut fixture = Fixture::new().await;
        fixture
            .controller
            .app_mut()
            .config
            .tui
            .terminal
            .notifications = TerminalNotificationMethod::Bell;
        fixture.controller.app_mut().config.tui.terminal.title = false;
        fixture.controller.app_mut().terminal_focused = false;
        fixture.controller.app_mut().snapshot.session_id = "loop-session".into();
        let mut agent = fixture.processor.agent_mut().take().unwrap();
        agent.session_id = "loop-session".into();
        let mut control = QueryTaskControl::new(agent.session_id.clone());
        control.turn_id = "loop-turn".into();
        let (release, released) = tokio::sync::oneshot::channel();
        let (finished, finished_rx) = tokio::sync::oneshot::channel();
        let (_events, receiver) = mpsc::unbounded_channel();
        fixture.controller.app_mut().bottom_pane.running_task = Some(RunningTask {
            kind: TaskKind::Query,
            receiver,
            handle: tokio::spawn(async move {
                released.await.unwrap();
                finished.send(()).unwrap();
                TaskCompletion::Query {
                    agent,
                    result: Ok(()),
                    goal_turn: None,
                }
            }),
            started_at: std::time::Instant::now(),
            next_heartbeat_after_secs: 100,
            cancellation_token: None,
            query_control: Some(control),
        });
        let port = fixture.port.clone();
        let screen = fixture.screen.clone();
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        emit(&port, 1, RuntimeEvent::Session(SessionEvent::TurnStarted));
        assert!(poll!(&mut future).is_pending());
        if terminal_first {
            emit(
                &port,
                2,
                RuntimeEvent::Session(SessionEvent::TurnFinished { reason: None }),
            );
            assert!(poll!(&mut future).is_pending());
            assert_eq!(count_bytes(&screen, b"\x07"), 0);
        }
        release.send(()).unwrap();
        finished_rx.await.unwrap();
        assert!(poll!(&mut future).is_pending());
        if !terminal_first {
            assert_eq!(count_bytes(&screen, b"\x07"), 0);
            emit(
                &port,
                2,
                RuntimeEvent::Session(SessionEvent::TurnFinished { reason: None }),
            );
            assert!(poll!(&mut future).is_pending());
        }
        assert_eq!(count_bytes(&screen, b"\x07"), 1);
        emit(
            &port,
            2,
            RuntimeEvent::Session(SessionEvent::TurnFinished { reason: None }),
        );
        assert!(poll!(&mut future).is_pending());
        assert_eq!(count_bytes(&screen, b"\x07"), 1);
    }
}

#[tokio::test]
async fn approval_notifications_follow_live_focus_without_replay_on_blur() {
    let mut fixture = Fixture::new().await;
    fixture
        .controller
        .app_mut()
        .config
        .tui
        .terminal
        .notifications = TerminalNotificationMethod::Osc9;
    fixture.controller.app_mut().config.tui.terminal.title = false;
    fixture.controller.app_mut().snapshot.session_id = "loop-session".into();
    fixture.processor.agent_mut().as_mut().unwrap().session_id = "loop-session".into();
    let screen = fixture.screen.clone();
    let input = fixture.input.clone();
    let port = fixture.port.clone();
    let future = fixture.run();
    tokio::pin!(future);
    assert!(poll!(&mut future).is_pending());
    emit(&port, 1, RuntimeEvent::Session(SessionEvent::TurnStarted));
    assert!(poll!(&mut future).is_pending());
    let request = |id: &str| {
        RuntimeEvent::Approval(ApprovalEvent::Requested {
            approval_id: id.into(),
            kind: "shell".into(),
        })
    };
    emit(&port, 2, request("first"));
    assert!(poll!(&mut future).is_pending());
    input.send(Ok(Event::FocusLost)).unwrap();
    assert!(poll!(&mut future).is_pending());
    emit(&port, 2, request("first"));
    assert!(poll!(&mut future).is_pending());
    assert_eq!(count_bytes(&screen, b"\x1b]9;"), 0);
    emit(&port, 3, request("second"));
    assert!(poll!(&mut future).is_pending());
    emit(&port, 3, request("second"));
    assert!(poll!(&mut future).is_pending());
    assert_eq!(
        count_bytes(&screen, b"\x1b]9;RARA: approval required\x07"),
        1
    );
    input.send(Ok(Event::FocusGained)).unwrap();
    assert!(poll!(&mut future).is_pending());
    input.send(Ok(Event::FocusLost)).unwrap();
    assert!(poll!(&mut future).is_pending());
    assert_eq!(count_bytes(&screen, b"\x1b]9;"), 1);
}
