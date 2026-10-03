use std::sync::Arc;

use rara_state::state_db::StateDb;
use rara_tools::tool::Tool;
use serde_json::json;

use super::{CreateGoalTool, UpdateGoalTool};
use crate::config::ConfigManager;
use crate::runtime_goals::{GoalStatus, RalphGoal};
use crate::tui::state::TuiApp;

#[tokio::test]
async fn tool_created_goal_survives_a_fresh_app() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.session_id = "durable-tool-thread".into();
    app.attach_state_db(db.clone());
    CreateGoalTool {
        store: app.goal_handle.clone(),
    }
    .call(json!({"objective": "finish the regression", "token_budget": 1234}))
    .await
    .expect("create goal");

    let saved = db.load_goal("durable-tool-thread").expect("durable goal");
    assert_eq!(saved["objective"], "finish the regression");
    assert_eq!(saved["token_budget"], 1234);
    drop(app);

    let mut restored = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("fresh app");
    restored.snapshot.session_id = "durable-tool-thread".into();
    restored.attach_state_db(db);
    assert_eq!(
        restored.goal.expect("restored goal").objective,
        "finish the regression"
    );
}

#[tokio::test]
async fn tool_status_changes_restore_exact_usage_and_original_creation_time() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    for (tool_status, expected) in [
        ("complete", GoalStatus::Complete),
        ("blocked", GoalStatus::Blocked),
    ] {
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .expect("app");
        app.snapshot.session_id = "tool-status-thread".into();
        app.attach_state_db(db.clone());
        let mut goal = RalphGoal::new("keep completed state".into(), Some(1000));
        goal.tokens_used = 777;
        goal.turns_completed = 4;
        goal.created_at_epoch_seconds = 1_234_567_890;
        app.goal_handle
            .replace(Some(goal.clone()))
            .expect("seed goal");
        UpdateGoalTool {
            store: app.goal_handle.clone(),
        }
        .call(json!({"status": tool_status}))
        .await
        .expect("update goal");
        goal.status = expected;
        drop(app);
        let mut restored = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .expect("fresh app");
        restored.snapshot.session_id = "tool-status-thread".into();
        restored.attach_state_db(db.clone());
        assert_eq!(restored.goal, Some(goal));
    }
}

#[tokio::test]
async fn tools_surface_write_failure_without_publishing_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.session_id = "failure-thread".into();
    app.attach_state_db(db.clone());
    let connection = rusqlite::Connection::open(db.path()).expect("independent connection");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_goal_write BEFORE INSERT ON goals
        BEGIN SELECT RAISE(FAIL, 'injected goal write failure'); END;",
        )
        .expect("failure trigger");
    let error = CreateGoalTool {
        store: app.goal_handle.clone(),
    }
    .call(json!({"objective": "do not publish this goal"}))
    .await
    .expect_err("write failure");
    assert!(error.to_string().contains("injected goal write failure"));
    assert!(app.goal_handle.snapshot().is_none());
    assert!(db.try_load_goal("failure-thread").expect("query").is_none());
}

#[tokio::test]
async fn unreadable_goal_binding_never_falls_back_to_in_memory_creation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let mut corrupt =
        serde_json::to_value(RalphGoal::new("corrupt goal".into(), None)).expect("serialize");
    corrupt["status"] = json!("Unknown");
    db.save_goal("corrupt-thread", &corrupt)
        .expect("seed corrupt row");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.session_id = "corrupt-thread".into();
    app.attach_state_db(db);
    assert!(app.goal.is_none());
    let error = CreateGoalTool {
        store: app.goal_handle.clone(),
    }
    .call(json!({"objective": "must not bypass broken persistence"}))
    .await
    .expect_err("unavailable binding");
    assert!(error.to_string().contains("goal persistence unavailable"));
    assert!(app.goal_handle.snapshot().is_none());
}
