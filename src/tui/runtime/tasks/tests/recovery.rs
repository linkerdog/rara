use super::*;

#[tokio::test]
async fn rebuild_config_save_failure_keeps_replacement_and_history() {
    let temp = tempdir().unwrap();
    let config_path = temp.path().join("config.json");
    let mut app = TuiApp::new(ConfigManager {
        path: config_path.clone(),
    })
    .unwrap();
    std::fs::create_dir(&config_path).unwrap();
    let mut previous = create_test_agent(&temp);
    previous.session_id = "existing-session".into();
    previous.history.push(Message {
        role: "user".into(),
        content: json!("keep history"),
    });
    let expected_history = previous.history.clone();
    let mut slot = Some(previous);
    install_completed_rebuild_task(&mut app, rebuild_success(&temp));
    let completion = (&mut app.bottom_pane.running_task.as_mut().unwrap().handle).await;
    super::super::finish_running_task_if_ready_from_runtime_port(
        &mut app,
        &mut slot,
        Some(completion),
        None,
    )
    .await
    .expect("save failure must not abort completion");
    let agent = slot.as_ref().expect("replacement installed");
    assert_eq!(agent.session_id, "existing-session");
    assert_eq!(agent.history, expected_history);
    assert!(!app.is_busy());
    assert_eq!(app.runtime_phase, RuntimePhase::BackendReady);
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .unwrap()
            .contains("not saved")
    );
    assert!(
        app.committed_turns
            .iter()
            .flat_map(|turn| &turn.entries)
            .any(|entry| entry.message.contains("not saved"))
    );
}

#[tokio::test]
async fn join_failure_clears_decisions_but_retains_queued_input() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .unwrap();
    install_completed_rebuild_task(&mut app, rebuild_success(&temp));
    app.bottom_pane.running_task.as_mut().unwrap().kind = TaskKind::Query;
    app.show_pending_plan_approval(Some("orphaned-plan"));
    app.queue_follow_up_message("keep queued input");
    let failure = tokio::spawn(async { panic!("scripted task panic") })
        .await
        .unwrap_err();
    super::super::finish_running_task_if_ready_from_runtime_port(
        &mut app,
        &mut None,
        Some(Err(failure)),
        None,
    )
    .await
    .expect("join failure must be recoverable");
    assert!(!app.is_busy());
    assert!(app.active_pending_interaction().is_none());
    assert!(app.has_queued_follow_up_messages());
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .unwrap()
            .contains("rebuild")
    );

    install_runtime_services(&mut app);
    let mut slot = None;
    assert_eq!(
        crate::tui::input_control::submit_user_prompt(&mut app, &mut slot, "Retry now".into()),
        crate::tui::input_control::InputControlOutcome::Queued,
    );
    let task = app
        .bottom_pane
        .running_task
        .as_mut()
        .expect("rebuild requested");
    assert!(matches!(task.kind, TaskKind::Rebuild));
    task.handle.abort();
    assert!(matches!((&mut task.handle).await, Err(error) if error.is_cancelled()));
    let mut rebuilt = rebuild_success(&temp);
    rebuilt.agent = create_test_agent_with_backend(&temp, Arc::new(PlainAnswerBackend));
    super::super::finish_running_task_if_ready_from_runtime_port(
        &mut app,
        &mut slot,
        Some(Ok(TaskCompletion::Rebuild {
            result: Ok(rebuilt),
        })),
        None,
    )
    .await
    .unwrap();
    let task = app
        .bottom_pane
        .running_task
        .as_mut()
        .expect("queued input restarted");
    assert!(matches!(task.kind, TaskKind::Query));
    let completion = tokio::time::timeout(Duration::from_secs(5), &mut task.handle)
        .await
        .unwrap()
        .unwrap();
    let TaskCompletion::Query { result, agent, .. } = &completion else {
        panic!("query completion")
    };
    assert!(result.is_ok(), "{result:?}");
    assert!(agent.history.iter().any(|message| message.role == "user"
        && message.content.to_string().contains("keep queued input")
        && message.content.to_string().contains("Retry now")));
    super::super::finish_running_task_if_ready_from_runtime_port(
        &mut app,
        &mut slot,
        Some(Ok(completion)),
        None,
    )
    .await
    .unwrap();
    assert!(slot.is_some());
    assert!(!app.is_busy());
    assert!(!app.has_queued_follow_up_messages());
    assert_eq!(app.runtime_phase, RuntimePhase::Idle);
}
