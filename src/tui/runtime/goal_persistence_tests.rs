use std::sync::Arc;

use rara_memory::memory_handle::MemoryHandle;
use rara_state::state_db::StateDb;
use rara_tools::tool::ToolManager;

use super::execute_local_command;
use crate::agent::Agent;
use crate::config::ConfigManager;
use crate::llm::MockLlm;
use crate::oauth::OAuthManager;
use crate::runtime_goals::RalphGoal;
use crate::session::SessionManager;
use crate::tui::state::{GoalStatus, LocalCommand, LocalCommandKind, TuiApp};
use crate::workspace::WorkspaceMemory;

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
    let data = dir.path().join("state");
    let mut ready_agent = Agent::new(
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
    ready_agent.set_session_id("command-thread".into());
    let mut agent = Some(ready_agent);
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
