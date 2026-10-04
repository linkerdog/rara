use std::sync::Arc;

use rara_memory::memory_handle::MemoryHandle;
use rara_state::state_db::StateDb;
use rara_tools::tool::ToolManager;

use super::{execute_local_command, execute_local_command_with_runtime};
use crate::agent::Agent;
use crate::config::ConfigManager;
use crate::llm::MockLlm;
use crate::oauth::OAuthManager;
use crate::runtime_goals::RalphGoal;
use crate::session::SessionManager;
use crate::tui::runtime_port::RuntimeCommand;
use crate::tui::state::{GoalStatus, LocalCommand, LocalCommandKind, TuiApp};
use crate::tui::testing::FakeRuntimeClient;
use crate::workspace::WorkspaceMemory;

#[tokio::test]
async fn goal_follow_up_rejects_missing_or_inactive_state_without_panicking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    let missing = super::start_goal_follow_up(&mut app, &mut None, None)
        .await
        .expect_err("missing goal");
    assert!(missing.to_string().contains("active goal"));
    for status in [
        GoalStatus::Paused,
        GoalStatus::Blocked,
        GoalStatus::Complete,
    ] {
        let mut goal = RalphGoal::new("guard follow-up".into(), None);
        goal.status = status;
        app.goal = Some(goal);
        let error = super::start_goal_follow_up(&mut app, &mut None, None)
            .await
            .expect_err("inactive goal");
        assert!(error.to_string().contains("inactive goals"));
    }
    app.goal = Some(RalphGoal::new("guard follow-up".into(), None));
    let missing = super::start_goal_follow_up(&mut app, &mut None, None)
        .await
        .expect_err("missing agent");
    assert!(missing.to_string().contains("ready runtime agent"));
    assert_eq!(
        app.goal.as_ref().expect("goal retained").status,
        GoalStatus::Pursuing
    );
}

#[tokio::test]
async fn goal_commands_persist_create_pause_and_clear_into_a_fresh_app() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let config_path = dir.path().join("config.json");
    let mut app = TuiApp::new(ConfigManager {
        path: config_path.clone(),
    })
    .expect("app");
    app.snapshot.session_id = "command-thread".into();
    app.attach_state_db(db.clone());
    let oauth =
        Arc::new(OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth"));
    let mut agent = None;
    for (command, status) in [
        (
            "--tokens 1000 preserve command state",
            Some(GoalStatus::Pursuing),
        ),
        ("pause", Some(GoalStatus::Paused)),
        ("clear", None),
    ] {
        execute_local_command(
            LocalCommand {
                kind: LocalCommandKind::Goal,
                arg: Some(command.into()),
            },
            &mut app,
            &mut agent,
            &oauth,
        )
        .await
        .expect("goal command");
        let mut fresh = TuiApp::new(ConfigManager {
            path: config_path.clone(),
        })
        .expect("fresh app");
        fresh.snapshot.session_id = "command-thread".into();
        fresh.attach_state_db(db.clone());
        assert_eq!(
            fresh.goal.as_ref().map(|goal| goal.status),
            status,
            "{command}"
        );
        assert_eq!(fresh.goal, app.goal);
    }
}

#[tokio::test]
async fn failed_goal_commands_keep_the_loop_and_committed_snapshot_alive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.session_id = "command-thread".into();
    app.attach_state_db(db.clone());
    let original = RalphGoal::new("retain committed goal".into(), None);
    app.goal_handle
        .replace(Some(original.clone()))
        .expect("seed goal");
    let stored = db.try_load_goal("command-thread").expect("seeded row");
    let connection = rusqlite::Connection::open(db.path()).expect("connection");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_goal_write BEFORE INSERT ON goals
         BEGIN SELECT RAISE(FAIL, 'injected goal write failure'); END;
         CREATE TRIGGER reject_goal_delete BEFORE DELETE ON goals
         BEGIN SELECT RAISE(FAIL, 'injected goal delete failure'); END;",
        )
        .expect("failure triggers");
    let oauth =
        Arc::new(OAuthManager::new_for_config_dir(dir.path().join("oauth")).expect("oauth"));
    let mut agent = Some(ready_agent(&dir));
    for command in ["pause", "clear"] {
        assert!(
            !execute_local_command(
                LocalCommand {
                    kind: LocalCommandKind::Goal,
                    arg: Some(command.into())
                },
                &mut app,
                &mut agent,
                &oauth,
            )
            .await
            .expect("goal failure must not propagate out of the command loop")
        );
        assert_eq!(app.goal, Some(original.clone()), "{command}");
        assert_eq!(
            app.goal_handle.snapshot(),
            Some(original.clone()),
            "{command}"
        );
        assert!(
            app.bottom_pane
                .notice
                .as_deref()
                .expect("failure notice")
                .contains("Goal command failed"),
            "{command}"
        );
        assert!(app.bottom_pane.running_task.is_none());
    }
    app.goal_handle
        .disable_after_persistence_failure("injected unavailable binding".into());
    assert!(
        !execute_local_command(
            LocalCommand {
                kind: LocalCommandKind::Goal,
                arg: Some("cannot create a memory-only goal".into())
            },
            &mut app,
            &mut agent,
            &oauth,
        )
        .await
        .expect("failed creation stays in the loop")
    );
    assert!(app.goal.is_none());
    assert!(app.goal_handle.snapshot().is_none());
    assert!(app.bottom_pane.running_task.is_none());
    assert!(agent.is_some(), "failed creation must not start a query");
    assert_eq!(
        db.try_load_goal("command-thread").expect("durable row"),
        stored
    );
}

#[tokio::test]
async fn resumed_goal_respects_persisted_budget_before_starting_a_turn() {
    for status in [GoalStatus::Paused, GoalStatus::Blocked] {
        for tokens_used in [9, 10, 15] {
            let dir = tempfile::tempdir().expect("tempdir");
            let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("db"));
            let mut app = TuiApp::new(ConfigManager {
                path: dir.path().join("config.json"),
            })
            .expect("app");
            app.snapshot.session_id = "command-thread".into();
            app.attach_state_db(db.clone());
            let mut goal = RalphGoal::new("respect resumed budget".into(), Some(10));
            goal.status = status;
            goal.tokens_used = tokens_used;
            app.goal_handle
                .replace(Some(goal.clone()))
                .expect("seed goal");
            let mut slot = Some(ready_agent(&dir));
            let runtime = FakeRuntimeClient::new(app.snapshot.clone().into_inner());
            execute_local_command_with_runtime(
                LocalCommand {
                    kind: LocalCommandKind::Goal,
                    arg: Some("resume".into()),
                },
                &mut app,
                &mut slot,
                Some(&runtime),
            )
            .await
            .expect("resume");
            goal.status = if tokens_used >= 10 {
                GoalStatus::BudgetLimited
            } else {
                GoalStatus::Pursuing
            };
            assert_eq!(app.goal, Some(goal.clone()));
            assert_eq!(app.goal_handle.snapshot(), Some(goal.clone()));
            let commands = runtime.commands();
            let [RuntimeCommand::ContinueGoal { ticket, mode }] = commands.as_slice() else {
                panic!("expected one goal query: {commands:?}")
            };
            assert_eq!(*mode, crate::runtime_goals::GoalContinuationMode::Requested);
            assert!(app.goal_handle.matches_resume_ticket(ticket));
            assert_eq!(
                app.goal_handle.claim_continuation(ticket, *mode).unwrap(),
                Some(goal.clone())
            );
            let stored: RalphGoal = serde_json::from_value(
                db.try_load_goal("command-thread")
                    .expect("load goal")
                    .expect("goal row"),
            )
            .expect("deserialize");
            assert_eq!(stored, goal);
        }
    }
}

#[tokio::test]
async fn failed_exhausted_resume_write_sends_no_wrap_up() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("db"));
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.session_id = "command-thread".into();
    app.attach_state_db(db.clone());
    let mut goal = RalphGoal::new("fail closed on budget write".into(), Some(10));
    goal.status = GoalStatus::Paused;
    goal.tokens_used = 10;
    app.goal_handle
        .replace(Some(goal.clone()))
        .expect("seed goal");
    rusqlite::Connection::open(db.path()).expect("connection").execute_batch("CREATE TRIGGER reject_goal_write BEFORE INSERT ON goals BEGIN SELECT RAISE(FAIL, 'injected budget status failure'); END;").expect("trigger");
    let mut slot = Some(ready_agent(&dir));
    let runtime = FakeRuntimeClient::new(app.snapshot.clone().into_inner());
    execute_local_command_with_runtime(
        LocalCommand {
            kind: LocalCommandKind::Goal,
            arg: Some("resume".into()),
        },
        &mut app,
        &mut slot,
        Some(&runtime),
    )
    .await
    .expect("visible command failure");
    assert_eq!(app.goal, Some(goal));
    assert!(runtime.commands().is_empty());
    assert!(slot.is_some());
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .expect("notice")
            .contains("injected budget status failure")
    );
}

fn ready_agent(dir: &tempfile::TempDir) -> Agent {
    let data = dir.path().join("state");
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(MockLlm),
        Arc::new(MemoryHandle::new(
            &data.join("memory").display().to_string(),
        )),
        Arc::new(SessionManager::new_for_rara_dir(data.clone()).expect("sessions")),
        Arc::new(WorkspaceMemory::from_paths(
            dir.path().join("workspace"),
            data,
        )),
    );
    agent.set_session_id("command-thread".into());
    agent
}
