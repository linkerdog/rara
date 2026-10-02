use std::sync::Arc;

use rara_memory::memory_handle::MemoryHandle;
use rara_state::state_db::{PersistedCompactState, PersistedPromptRuntimeState, StateDb};
use rara_tools::tool::ToolManager;

use super::restore_thread_by_id;
use crate::agent::Agent;
use crate::config::ConfigManager;
use crate::llm::MockLlm;
use crate::runtime_goals::{GoalStatus, RalphGoal};
use crate::session::SessionManager;
use crate::tui::state::TuiApp;
use crate::workspace::WorkspaceMemory;

#[test]
fn thread_restore_round_trips_every_goal_status_and_does_not_revive_clear() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("workspace");
    let data = dir.path().join("state");
    let sessions = Arc::new(SessionManager::new_for_rara_dir(data.clone()).expect("sessions"));
    let db = Arc::new(StateDb::new_for_root_dir(data.clone()).expect("state db"));
    db.upsert_session(
        "stored-thread",
        &root.display().to_string(),
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
    .expect("thread metadata");
    for status in [
        GoalStatus::Pursuing,
        GoalStatus::Paused,
        GoalStatus::Blocked,
        GoalStatus::Complete,
        GoalStatus::BudgetLimited,
    ] {
        let mut expected = RalphGoal::new("preserve restored thread goal".into(), Some(1000));
        expected.status = status;
        expected.tokens_used = 456;
        expected.turns_completed = 9;
        expected.created_at_epoch_seconds = 1_234_567_890;
        db.save_goal(
            "stored-thread",
            &serde_json::to_value(&expected).expect("serialize"),
        )
        .expect("seed goal");
        let agent = Agent::new(
            ToolManager::new(),
            Arc::new(MockLlm),
            Arc::new(MemoryHandle::new(
                &data.join("memory").display().to_string(),
            )),
            sessions.clone(),
            Arc::new(WorkspaceMemory::from_paths(root.clone(), data.clone())),
        );
        let mut slot = Some(agent);
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .expect("fresh app");
        app.attach_state_db(db.clone());
        restore_thread_by_id("stored-thread", &mut app, &mut slot).expect("restore thread");
        assert_eq!(app.goal, Some(expected.clone()));
        assert_eq!(app.goal_handle.snapshot(), Some(expected));
        assert_eq!(app.snapshot.session_id, "stored-thread");

        app.goal_handle
            .replace(None)
            .expect("clear through runtime writer");
        app.goal = Some(RalphGoal::new("stale prior thread goal".into(), None));
        restore_thread_by_id("stored-thread", &mut app, &mut slot).expect("restore cleared thread");
        assert!(app.goal.is_none());
        assert!(app.goal_handle.snapshot().is_none());
    }
}
