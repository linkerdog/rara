use std::sync::Arc;

use rara_state::state_db::StateDb;

use super::execute_local_command;
use crate::config::ConfigManager;
use crate::oauth::OAuthManager;
use crate::tui::state::{GoalStatus, LocalCommand, LocalCommandKind, TuiApp};

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
