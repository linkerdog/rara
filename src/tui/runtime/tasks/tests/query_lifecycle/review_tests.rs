use super::*;
use crate::tui::message_role::MessageRole;

#[tokio::test]
async fn broadcast_boundary_does_not_replay_future_query_receipts() {
    let mut fixture = Fixture::start(BackendOutcome::Answer).await;
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    let events = fixture.drain_events();
    let first_delta = events
        .iter()
        .position(|event| {
            matches!(
                &event.event,
                RuntimeEvent::Assistant(AssistantEvent::TextDelta(text)) if text == "Before. "
            )
        })
        .expect("first response delta");
    for event in events.into_iter().take(first_delta + 1) {
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
    let text = fixture
        .controller
        .app()
        .committed_turns
        .iter()
        .flat_map(|turn| &turn.entries)
        .filter(|entry| entry.role == MessageRole::Agent)
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text, "Before. Tail.");
}

#[tokio::test]
async fn cancel_after_execution_return_preserves_the_new_plan_approval() {
    let mut fixture = Fixture::start(BackendOutcome::PlanApproval).await;
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    let TaskCompletion::Query { agent, result, .. } = completion.as_ref().as_ref().unwrap() else {
        panic!("expected query completion");
    };
    assert!(result.is_ok());
    assert!(agent.has_pending_plan_exit_approval());
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
            .unwrap()
    );
    assert!(fixture.controller.app().has_pending_plan_approval());
    assert!(
        fixture
            .processor
            .agent()
            .unwrap()
            .has_pending_plan_exit_approval()
    );
    assert_eq!(
        fixture.controller.app().runtime_phase_detail.as_deref(),
        Some("awaiting plan approval")
    );
}

#[tokio::test]
async fn lagged_terminal_is_recovered_before_a_later_catalog_sequence() {
    let mut fixture = Fixture::start(BackendOutcome::Answer).await;
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    let bus = fixture.processor.event_bus();
    for _ in 0..300 {
        bus.publish_control(RuntimeEvent::Input(
            crate::runtime_control::InputEvent::UserPromptSubmitted,
        ));
    }
    assert!(matches!(
        fixture.events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
    ));
    let later = fixture
        .events
        .try_recv()
        .expect("catalog after the lost query events");
    assert!(later.turn_id.is_none());
    assert!(fixture.deliver(later).await);
    assert!(
        fixture.controller.app().bottom_pane.running_task.is_some(),
        "terminal evidence does not replace the task join"
    );
    assert!(
        fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion)
            .await
            .unwrap()
    );
    let text = fixture
        .controller
        .app()
        .committed_turns
        .iter()
        .flat_map(|turn| &turn.entries)
        .filter(|entry| entry.role == MessageRole::Agent)
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Before. Tail."), "{text}");
    assert_eq!(text.matches("Before.").count(), 1);
    assert!(fixture.controller.app().bottom_pane.running_task.is_none());
}

#[tokio::test]
async fn accepted_stop_discards_new_pending_interactions() {
    let mut fixture = Fixture::start(BackendOutcome::Answer).await;
    fixture
        .controller
        .apply_runtime_command(
            &mut fixture.processor,
            RuntimeCommand::Session(SessionControlRequest::CancelCurrentTurn),
        )
        .await
        .unwrap();
    fixture.backend.release.notify_one();
    let mut completion = fixture.task_return().await;
    let TaskCompletion::Query { agent, result, .. } = completion.as_mut().as_mut().unwrap() else {
        panic!("expected query completion");
    };
    assert!(result.is_err());
    // Model the finalizer observing a just-raised interaction after stop admission.
    agent.restore_pending_plan_exit_approval("exit-plan");
    agent.pending_user_input = Some(crate::agent::PendingUserInput {
        question: "Choose a path".into(),
        options: Vec::new(),
        note: None,
    });
    fixture
        .controller
        .app_mut()
        .show_pending_plan_approval(Some("exit-plan"));
    assert!(
        fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion)
            .await
            .unwrap()
    );
    let agent = fixture.processor.agent().unwrap();
    assert!(!agent.has_pending_plan_exit_approval());
    assert!(agent.pending_user_input.is_none());
    assert!(!fixture.controller.app().has_pending_plan_approval());
    assert_eq!(
        fixture.controller.app().runtime_phase_detail.as_deref(),
        Some("query cancelled")
    );
}

#[tokio::test]
async fn review_regression_dropped_broadcast_recovers_tail_and_completion() {
    let mut fixture = Fixture::start(BackendOutcome::Answer).await;
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    let dropped = fixture.drain_events();
    assert!(dropped.iter().any(|event| matches!(
        event.event,
        RuntimeEvent::Session(SessionEvent::TurnFinished { .. })
    )));
    assert!(
        fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion,)
            .await
            .unwrap()
    );
    assert!(fixture.controller.app().bottom_pane.running_task.is_none());
    let text = fixture
        .controller
        .app()
        .committed_turns
        .iter()
        .flat_map(|turn| &turn.entries)
        .filter(|entry| entry.role == MessageRole::Agent)
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Before. Tail."), "{text}");
    assert_eq!(text.matches("Before.").count(), 1);
    for event in dropped {
        assert!(
            !fixture.deliver(event).await,
            "recovered query event applied twice"
        );
    }
}

#[tokio::test]
async fn review_regression_compact_events_survive_a_completed_query() {
    let mut fixture = Fixture::start(BackendOutcome::Answer).await;
    fixture.backend.release.notify_one();
    let completion = fixture.task_return().await;
    for event in fixture.drain_events() {
        assert!(fixture.deliver(event).await);
    }
    assert!(
        fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion,)
            .await
            .unwrap()
    );
    fixture
        .controller
        .apply_runtime_command(
            &mut fixture.processor,
            RuntimeCommand::Maintenance(
                crate::tui::runtime_port::RuntimeMaintenanceCommand::Compact,
            ),
        )
        .await
        .unwrap();
    let completion = fixture.task_return().await;
    let events = fixture.drain_events();
    assert!(events.iter().any(|event| matches!(
        event.event,
        RuntimeEvent::Session(SessionEvent::TurnStarted)
    )));
    assert!(events.iter().any(|event| matches!(
        event.event,
        RuntimeEvent::Session(SessionEvent::TurnFinished { .. })
    )));
    for event in events {
        assert!(event.turn_id.is_none());
        assert!(
            fixture.deliver(event).await,
            "maintenance event was fenced out"
        );
    }
    assert!(
        fixture
            .controller
            .receive_runtime_task_completion(&mut fixture.processor, completion,)
            .await
            .unwrap()
    );
    assert!(fixture.processor.agent().is_some());
}
