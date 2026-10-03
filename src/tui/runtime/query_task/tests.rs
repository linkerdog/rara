use super::*;

#[test]
fn review_regression_stop_preserves_execution_error() {
    let control = QueryTaskControl::new("session".into());
    let token = AtomicBool::new(false);
    control.request_stop(QueryStopKind::Cancel, &token);
    let bus = RuntimeEventBus::new(8);
    let mut events = bus.subscribe_control();
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = control
        .publish_finished(&bus, &sender, Err(anyhow::anyhow!("provider disconnected")))
        .expect_err("stop preserves the execution failure");
    assert!(format!("{error:#}").contains("provider disconnected"));
    assert!(matches!(
        events.try_recv().unwrap().event,
        RuntimeEvent::Error(ErrorEvent::RuntimeError { message, .. })
            if message.contains("provider disconnected")
    ));
    assert_eq!(
        events.try_recv().unwrap().event,
        RuntimeEvent::Session(SessionEvent::TurnCancelled)
    );
}

#[test]
fn first_stop_kind_survives_repeat_requests_and_successful_execution_return() {
    for kind in [QueryStopKind::Cancel, QueryStopKind::Interrupt] {
        let control = QueryTaskControl::new("session".into());
        let token = AtomicBool::new(false);
        assert_eq!(
            control.request_stop(kind, &token),
            QueryStopRequest::Requested
        );
        for next in [QueryStopKind::Cancel, QueryStopKind::Interrupt] {
            assert_eq!(
                control.request_stop(next, &token),
                QueryStopRequest::AlreadyRequested
            );
            assert_eq!(control.stop_kind(), Some(kind));
        }
        let (result, terminal) = control.finish(Ok(()));
        assert!(result.is_err());
        assert_eq!(
            terminal,
            match kind {
                QueryStopKind::Cancel => SessionEvent::TurnCancelled,
                QueryStopKind::Interrupt => SessionEvent::TurnInterrupted,
            }
        );
        assert_eq!(
            control.request_stop(kind, &token),
            QueryStopRequest::Finished
        );
    }
}

#[test]
fn completed_execution_rejects_stop_without_signalling_token() {
    let control = QueryTaskControl::new("session".into());
    let token = AtomicBool::new(false);
    let (result, terminal) = control.finish(Ok(()));
    assert!(result.is_ok());
    assert!(matches!(terminal, SessionEvent::TurnFinished { .. }));
    assert_eq!(
        control.request_stop(QueryStopKind::Cancel, &token),
        QueryStopRequest::Finished
    );
    assert!(!token.load(Ordering::SeqCst));
}

#[test]
fn concurrent_stop_and_return_have_one_consistent_outcome() {
    for _ in 0..64 {
        let control = QueryTaskControl::new("session".into());
        let token = AtomicBool::new(false);
        let start = std::sync::Barrier::new(2);
        let (request, (result, terminal)) = std::thread::scope(|scope| {
            let stop = scope.spawn(|| {
                start.wait();
                control.request_stop(QueryStopKind::Cancel, &token)
            });
            let finish = scope.spawn(|| {
                start.wait();
                control.finish(Ok(()))
            });
            (stop.join().unwrap(), finish.join().unwrap())
        });
        match request {
            QueryStopRequest::Requested => {
                assert!(result.is_err());
                assert_eq!(terminal, SessionEvent::TurnCancelled);
                assert!(token.load(Ordering::SeqCst));
            }
            QueryStopRequest::Finished => {
                assert!(result.is_ok());
                assert!(matches!(terminal, SessionEvent::TurnFinished { .. }));
                assert!(!token.load(Ordering::SeqCst));
            }
            QueryStopRequest::AlreadyRequested => panic!("there is only one stop request"),
        }
    }
}

#[test]
fn intermediate_nonrecoverable_diagnostic_is_preserved_without_ending_query() {
    let control = QueryTaskControl::new("session".into());
    let bus = RuntimeEventBus::new(8);
    let mut events = bus.subscribe_control();
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut pending_error = None;
    control.publish_dispatch_event(
        &bus,
        &sender,
        &mut pending_error,
        crate::runtime_control::wrap_agent_event(
            "diagnostic",
            0,
            RuntimeProvenance::local_tui("session"),
            AgentEvent::AgentError {
                message: "stop hook reached its continuation limit".into(),
                recoverable: false,
            },
        ),
    );
    assert!(
        events.try_recv().is_err(),
        "diagnostic is held until the next dispatch event"
    );
    control.publish_dispatch_event(
        &bus,
        &sender,
        &mut pending_error,
        crate::runtime_control::wrap_agent_event(
            "dispatch-stop",
            0,
            RuntimeProvenance::local_tui("session"),
            AgentEvent::AgentStop {
                reason: "turn complete".into(),
            },
        ),
    );
    let event = events.try_recv().unwrap();
    assert!(matches!(
        event.event,
        RuntimeEvent::Error(ErrorEvent::RuntimeError {
            recoverable: false,
            ..
        })
    ));
    assert_eq!(event.turn_id.as_deref(), Some(control.turn_id.as_str()));
    assert!(events.try_recv().is_err());
    assert!(control.publish_finished(&bus, &sender, Ok(())).is_ok());
    assert!(matches!(
        events.try_recv().unwrap().event,
        RuntimeEvent::Session(SessionEvent::TurnFinished { .. })
    ));
}

#[test]
fn final_dispatch_error_is_replaced_by_one_execution_owned_diagnostic() {
    let control = QueryTaskControl::new("session".into());
    let bus = RuntimeEventBus::new(8);
    let mut events = bus.subscribe_control();
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut pending_error = None;
    control.publish_dispatch_event(
        &bus,
        &sender,
        &mut pending_error,
        crate::runtime_control::wrap_agent_event(
            "dispatch-error",
            0,
            RuntimeProvenance::local_tui("session"),
            AgentEvent::AgentError {
                message: "provider failed".into(),
                recoverable: false,
            },
        ),
    );
    assert!(events.try_recv().is_err());
    assert!(
        control
            .publish_finished(&bus, &sender, Err(anyhow::anyhow!("provider failed")))
            .is_err()
    );
    assert!(matches!(
        events.try_recv().unwrap().event,
        RuntimeEvent::Error(ErrorEvent::RuntimeError {
            recoverable: false,
            ..
        })
    ));
    assert!(matches!(
        events.try_recv().unwrap().event,
        RuntimeEvent::Session(SessionEvent::TurnFailed { .. })
    ));
    assert!(events.try_recv().is_err());
}
