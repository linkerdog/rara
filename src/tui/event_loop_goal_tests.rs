use rara_state::state_db::{PersistedCompactState, PersistedPromptRuntimeState, StateDb};

use super::*;
use crate::runtime_goals::{GoalStatus, RalphGoal};
use crate::tui::state::{ListPickerKind, Overlay};

fn seed_thread(fixture: &mut Fixture, status: GoalStatus) {
    let root = fixture._dir.path();
    let db = Arc::new(StateDb::new_for_root_dir(root.join("thread-db")).unwrap());
    db.upsert_session(
        "resumed-thread",
        root.to_str().unwrap(),
        "main",
        "ollama",
        "test",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        0,
        &PersistedCompactState::default(),
    )
    .unwrap();
    let mut goal = RalphGoal::new("resume the saved objective".into(), None);
    goal.status = status;
    db.save_goal("resumed-thread", &serde_json::to_value(goal).unwrap())
        .unwrap();
    fixture.controller.app_mut().snapshot.cwd = root.to_string_lossy().into_owned();
    fixture.controller.app_mut().attach_state_db(db);
}

async fn restore(fixture: &mut Fixture) {
    crate::tui::session_restore::restore_thread_by_id(
        "resumed-thread",
        fixture.controller.app_mut(),
        fixture.processor.agent_mut(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn refused_or_failed_goal_admission_keeps_the_ready_agent() {
    for status in [GoalStatus::Paused, GoalStatus::Pursuing] {
        let mut fixture = Fixture::new().await;
        seed_thread(&mut fixture, status);
        restore(&mut fixture).await;
        let expected_session = fixture.processor.agent().unwrap().session_id.clone();
        let app = fixture.controller.app_mut();
        if status == GoalStatus::Pursuing {
            app.goal_handle
                .mutate(|goal| {
                    let goal = goal.as_mut().unwrap();
                    goal.token_budget = Some(1);
                    goal.tokens_used = 1;
                    Ok(())
                })
                .unwrap();
            let db = app.state_db.as_ref().unwrap();
            rusqlite::Connection::open(db.path()).unwrap().execute_batch(
                "CREATE TRIGGER reject_goal_write BEFORE INSERT ON goals BEGIN SELECT RAISE(FAIL, 'injected admission failure'); END;"
            ).unwrap();
        }
        let expected_goal = app.goal_handle.snapshot();
        let ticket = app.goal_handle.resume_ticket().unwrap();
        fixture
            .processor
            .apply_command(
                app,
                RuntimeCommand::ContinueGoal {
                    ticket,
                    mode: crate::runtime_goals::GoalContinuationMode::Automatic,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.processor.agent().unwrap().session_id,
            expected_session
        );
        assert_eq!(app.goal_handle.snapshot(), expected_goal);
        assert!(app.bottom_pane.running_task.is_none());
        assert!(app.pending_goal_resume.is_none());
        if status == GoalStatus::Pursuing {
            assert!(
                app.notice_text()
                    .unwrap()
                    .contains("injected admission failure")
            );
        }
    }
}

#[tokio::test]
async fn plan_mode_retains_automatic_goal_until_execute_admission() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Pursuing);
    restore(&mut fixture).await;
    fixture.controller.app_mut().agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    let ticket = fixture
        .controller
        .app()
        .goal_handle
        .resume_ticket()
        .unwrap();
    fixture
        .controller
        .queue_restored_goal(&fixture.processor)
        .await;
    assert!(fixture.port.commands().is_empty());
    fixture
        .processor
        .apply_command(
            fixture.controller.app_mut(),
            RuntimeCommand::ContinueGoal {
                ticket,
                mode: crate::runtime_goals::GoalContinuationMode::Automatic,
            },
        )
        .await
        .unwrap();
    assert!(fixture.processor.agent().is_some());
    assert!(
        fixture
            .controller
            .app()
            .pending_goal_resume
            .as_ref()
            .is_some_and(|pending| !pending.enqueued)
    );
    fixture.controller.app_mut().agent_execution_mode = crate::agent::AgentExecutionMode::Execute;
    fixture
        .controller
        .queue_restored_goal(&fixture.processor)
        .await;
    let commands = fixture.port.commands();
    assert_eq!(commands.len(), 1);
    fixture
        .processor
        .apply_command(fixture.controller.app_mut(), commands[0].clone())
        .await
        .unwrap();
    assert!(fixture.controller.app().pending_goal_resume.is_none());
    let task = fixture
        .controller
        .app_mut()
        .bottom_pane
        .running_task
        .take()
        .unwrap();
    assert!(matches!(
        task.handle.await.unwrap(),
        TaskCompletion::Query { .. }
    ));
}

#[tokio::test]
async fn actual_loop_queues_restored_goal_once_without_a_keypress() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Pursuing);
    restore(&mut fixture).await;
    let port = fixture.port.clone();
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        assert!(matches!(
            port.commands().as_slice(),
            [RuntimeCommand::ContinueGoal {
                mode: crate::runtime_goals::GoalContinuationMode::Automatic,
                ..
            }]
        ));
        for _ in 0..4 {
            advance(Duration::from_millis(166)).await;
            assert!(poll!(&mut future).is_pending());
        }
        assert_eq!(port.commands().len(), 1);
    }
}

#[tokio::test]
async fn picker_and_latest_restore_arm_the_same_idle_continuation() {
    for picker in [false, true] {
        let mut fixture = Fixture::new().await;
        seed_thread(&mut fixture, GoalStatus::Pursuing);
        if picker {
            fixture
                .controller
                .app_mut()
                .open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
            fixture
                .controller
                .app_mut()
                .finish_resume_query_for_test()
                .await;
            fixture
                .controller
                .dispatch_event(
                    &mut fixture.processor,
                    crate::tui::app_event::AppEvent::ApplyOverlaySelection,
                    &fixture.oauth,
                )
                .await
                .unwrap();
            crate::tui::session_restore::finish_restore_for_test(
                fixture.controller.app_mut(),
                fixture.processor.agent_mut(),
            )
            .await
            .unwrap();
        } else {
            let db = fixture.controller.app().state_db.as_ref().unwrap().clone();
            crate::tui::session_restore::restore_latest_thread(
                &db,
                fixture.controller.app_mut(),
                fixture.processor.agent_mut(),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            fixture.controller.app().snapshot.session_id,
            "resumed-thread"
        );
        fixture
            .controller
            .queue_restored_goal(&fixture.processor)
            .await;
        assert!(matches!(
            fixture.port.commands().as_slice(),
            [RuntimeCommand::ContinueGoal {
                mode: crate::runtime_goals::GoalContinuationMode::Automatic,
                ..
            }]
        ));
    }
}

#[tokio::test]
async fn restored_command_rechecks_readiness_and_claims_only_once() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Pursuing);
    restore(&mut fixture).await;
    fixture
        .controller
        .apply_runtime_command(
            &mut fixture.processor,
            RuntimeCommand::SetPermissionMode(PermissionMode::Auto),
        )
        .await
        .unwrap();
    fixture
        .controller
        .queue_restored_goal(&fixture.processor)
        .await;
    let command = fixture.port.commands()[0].clone();
    let agent = fixture.processor.agent_mut().take();
    fixture
        .controller
        .apply_runtime_command(&mut fixture.processor, command.clone())
        .await
        .unwrap();
    assert!(
        fixture
            .controller
            .app()
            .pending_goal_resume
            .as_ref()
            .is_some_and(|pending| !pending.enqueued)
    );
    assert!(fixture.controller.app().bottom_pane.running_task.is_none());
    *fixture.processor.agent_mut() = agent;
    fixture
        .controller
        .queue_restored_goal(&fixture.processor)
        .await;
    fixture
        .controller
        .apply_runtime_command(&mut fixture.processor, command.clone())
        .await
        .unwrap();
    let control = fixture
        .controller
        .app()
        .bottom_pane
        .running_task
        .as_ref()
        .unwrap()
        .query_control
        .as_ref()
        .unwrap()
        .turn_id
        .clone();
    fixture
        .controller
        .apply_runtime_command(&mut fixture.processor, command)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .controller
            .app()
            .bottom_pane
            .running_task
            .as_ref()
            .unwrap()
            .query_control
            .as_ref()
            .unwrap()
            .turn_id,
        control
    );
    let app = fixture.controller.app();
    let notice = app.notice_text().unwrap();
    for text in [
        "Resuming goal: resume the saved objective",
        "/goal pause",
        "permissions: auto",
    ] {
        assert!(notice.contains(text), "{notice}");
    }
    let task = fixture
        .controller
        .app_mut()
        .bottom_pane
        .running_task
        .take()
        .unwrap();
    let TaskCompletion::Query { agent, .. } = task.handle.await.unwrap() else {
        panic!("query completion")
    };
    assert!(!agent.full_access_mode);
}

#[tokio::test]
async fn stale_queued_goal_does_not_take_the_agent() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Pursuing);
    restore(&mut fixture).await;
    fixture
        .controller
        .queue_restored_goal(&fixture.processor)
        .await;
    let command = fixture.port.commands()[0].clone();
    fixture
        .controller
        .app()
        .goal_handle
        .record_turn_started()
        .unwrap();
    fixture
        .controller
        .apply_runtime_command(&mut fixture.processor, command)
        .await
        .unwrap();
    assert!(fixture.processor.agent().is_some());
    assert!(fixture.controller.app().bottom_pane.running_task.is_none());
}

#[tokio::test]
async fn accepted_user_cancel_defers_goal_across_thread_restore() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Pursuing);
    restore(&mut fixture).await;
    fixture
        .controller
        .queue_restored_goal(&fixture.processor)
        .await;
    fixture
        .controller
        .apply_runtime_command(&mut fixture.processor, fixture.port.commands()[0].clone())
        .await
        .unwrap();
    fixture
        .controller
        .apply_runtime_command(
            &mut fixture.processor,
            RuntimeCommand::Session(
                crate::runtime_control::SessionControlRequest::CancelCurrentTurn,
            ),
        )
        .await
        .unwrap();
    assert!(fixture.controller.app().goal_handle.continuation_deferred());
    let task = fixture
        .controller
        .app_mut()
        .bottom_pane
        .running_task
        .take()
        .unwrap();
    let TaskCompletion::Query { agent, .. } = task.handle.await.unwrap() else {
        panic!("query completion")
    };
    *fixture.processor.agent_mut() = Some(agent);
    restore(&mut fixture).await;
    assert!(fixture.controller.app().goal_handle.continuation_deferred());
    assert!(fixture.controller.app().pending_goal_resume.is_none());
    let count = fixture.port.commands().len();
    fixture
        .controller
        .queue_restored_goal(&fixture.processor)
        .await;
    assert_eq!(fixture.port.commands().len(), count);
}

#[tokio::test]
async fn paused_goal_resume_choice_requires_explicit_acceptance() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Paused);
    restore(&mut fixture).await;
    fixture
        .controller
        .queue_restored_goal(&fixture.processor)
        .await;
    assert!(fixture.port.commands().is_empty());
    assert_eq!(fixture.controller.app().overlay, Some(Overlay::Goal));
    fixture
        .controller
        .dispatch_event(
            &mut fixture.processor,
            crate::tui::app_event::AppEvent::Goal(crate::tui::goal_ui::GoalUiAction::Accept),
            &fixture.oauth,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .controller
            .app()
            .goal_handle
            .snapshot()
            .unwrap()
            .status,
        GoalStatus::Pursuing
    );
    assert!(matches!(
        fixture.port.commands().as_slice(),
        [RuntimeCommand::ContinueGoal {
            mode: crate::runtime_goals::GoalContinuationMode::Requested,
            ..
        }]
    ));
}

#[tokio::test]
async fn fresh_bootstrap_with_a_goal_does_not_imply_explicit_resume() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Pursuing);
    let app = fixture.controller.app_mut();
    app.goal = app
        .goal_handle
        .restore_for_thread("resumed-thread", app.state_db.as_ref().unwrap().clone())
        .unwrap();
    let port = fixture.port.clone();
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        advance(Duration::from_millis(200)).await;
        assert!(poll!(&mut future).is_pending());
        assert!(port.commands().is_empty());
    }
}

#[tokio::test]
async fn startup_plugin_rebuild_waits_for_prepared_restore() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Paused);
    let (release, wait) = std::sync::mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let blocked = fixture
        .controller
        .app()
        .storage
        .as_ref()
        .unwrap()
        .read(move || {
            entered.send(()).unwrap();
            wait.recv()?;
            Ok(())
        })
        .unwrap();
    ready.await.unwrap();
    crate::tui::session_restore::request_restore_thread(
        "resumed-thread",
        fixture.controller.app_mut(),
        fixture.processor.agent_mut(),
    )
    .unwrap();
    let prepared = fixture
        .controller
        .app()
        .storage
        .as_ref()
        .unwrap()
        .read(|| Ok(()))
        .unwrap();
    let port = fixture.port.clone();
    tokio::time::pause();
    {
        let future = run_event_loop(
            &mut fixture.terminal,
            &mut fixture.controller,
            &mut fixture.processor,
            &fixture.oauth,
            &mut fixture.source,
            crate::tui::event_loop::StartupMaintenance::Rebuild,
        );
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        assert!(
            port.commands().is_empty(),
            "startup maintenance must wait for the session binding"
        );
        release.send(()).unwrap();
        blocked.await.unwrap().unwrap();
        prepared.await.unwrap().unwrap();
        advance(Duration::from_millis(166)).await;
        assert!(poll!(&mut future).is_pending());
        assert!(matches!(
            port.commands().as_slice(),
            [RuntimeCommand::Maintenance(
                crate::tui::runtime_port::RuntimeMaintenanceCommand::Rebuild
            )]
        ));
    }
    assert_eq!(
        fixture.processor.agent().unwrap().session_id,
        "resumed-thread"
    );
    tokio::time::resume();
    fixture
        .controller
        .app_mut()
        .shutdown_storage()
        .await
        .unwrap();
}

#[tokio::test]
async fn startup_resume_does_not_index_the_unused_fresh_session() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Paused);
    let fresh_id = fixture.processor.agent().unwrap().session_id.clone();
    let db = fixture.controller.app().state_db.clone().unwrap();
    crate::tui::session_restore::apply_startup_resume(
        &crate::tui::event_loop::StartupResumeTarget::Latest,
        fixture.controller.app_mut(),
        fixture.processor.agent_mut(),
    );
    fixture
        .processor
        .sync_snapshot(fixture.controller.app_mut());
    fixture.controller.app_mut().flush_storage().await.unwrap();
    assert!(
        db.load_session_runtime_state(&fresh_id).unwrap().is_none(),
        "the provisional snapshot must not become a more recent empty thread"
    );
    crate::tui::session_restore::finish_restore_for_test(
        fixture.controller.app_mut(),
        fixture.processor.agent_mut(),
    )
    .await
    .unwrap();
    assert_eq!(
        fixture.processor.agent().unwrap().session_id,
        "resumed-thread"
    );
    fixture
        .controller
        .app_mut()
        .shutdown_storage()
        .await
        .unwrap();
    assert!(db.load_session_runtime_state(&fresh_id).unwrap().is_none());
}
