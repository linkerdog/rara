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

fn restore(fixture: &mut Fixture) {
    crate::tui::session_restore::restore_thread_by_id(
        "resumed-thread",
        fixture.controller.app_mut(),
        fixture.processor.agent_mut(),
    )
    .unwrap();
}

#[tokio::test]
async fn actual_loop_queues_restored_goal_once_without_a_keypress() {
    let mut fixture = Fixture::new().await;
    seed_thread(&mut fixture, GoalStatus::Pursuing);
    restore(&mut fixture);
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
                .dispatch_event(
                    &mut fixture.processor,
                    crate::tui::app_event::AppEvent::ApplyOverlaySelection,
                    &fixture.oauth,
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
    restore(&mut fixture);
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
    let notice = app.bottom_pane.notice.as_deref().unwrap();
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
    restore(&mut fixture);
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
    restore(&mut fixture);
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
    restore(&mut fixture);
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
    restore(&mut fixture);
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
