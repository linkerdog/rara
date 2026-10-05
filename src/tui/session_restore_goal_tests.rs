use std::sync::Arc;

use rara_memory::memory_handle::MemoryHandle;
use rara_state::state_db::{PersistedCompactState, PersistedPromptRuntimeState, StateDb};
use rara_tools::tool::ToolManager;
use serde_json::json;

use super::{restore_latest_thread, restore_thread_by_id};
use crate::agent::Agent;
use crate::config::ConfigManager;
use crate::llm::{Message, MockLlm};
use crate::runtime_goals::{GoalStatus, RalphGoal};
use crate::session::SessionManager;
use crate::tui::state::TuiApp;
use crate::workspace::WorkspaceMemory;

#[tokio::test]
async fn thread_restore_preserves_statuses_counter_limits_and_clear() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("workspace");
    let data = dir.path().join("state");
    let sessions = Arc::new(SessionManager::new_for_rara_dir(data.clone()).expect("sessions"));
    let db = Arc::new(StateDb::new_for_root_dir(data.clone()).expect("state db"));
    seed_thread(&db, "stored-thread", &root);
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
        if status == GoalStatus::Paused {
            expected.token_budget = Some(u32::MAX);
            expected.tokens_used = u32::MAX;
            expected.turns_completed = u32::MAX;
        }
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
        restore_thread_by_id("stored-thread", &mut app, &mut slot)
            .await
            .expect("restore thread");
        assert_eq!(app.goal, Some(expected.clone()));
        assert_eq!(app.goal_handle.snapshot(), Some(expected));
        assert_eq!(app.snapshot.session_id, "stored-thread");

        app.goal_handle
            .replace(None)
            .expect("clear through runtime writer");
        app.goal = Some(RalphGoal::new("stale prior thread goal".into(), None));
        restore_thread_by_id("stored-thread", &mut app, &mut slot)
            .await
            .expect("restore cleared thread");
        assert!(app.goal.is_none());
        assert!(app.goal_handle.snapshot().is_none());
    }
}

#[tokio::test]
async fn failed_thread_reads_preserve_agent_snapshot_and_goal_binding() {
    for corrupt in ["todo", "runtime"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("workspace");
        let data = dir.path().join("state");
        let sessions = Arc::new(SessionManager::new_for_rara_dir(data.clone()).expect("sessions"));
        let db = Arc::new(StateDb::new_for_root_dir(data.clone()).expect("state db"));
        seed_thread(&db, "target-thread", &root);
        let target = RalphGoal::new("target goal".into(), None);
        db.save_goal(
            "target-thread",
            &serde_json::to_value(&target).expect("serialize"),
        )
        .expect("target goal");
        let target_row = db.try_load_goal("target-thread").expect("target row");
        let mut agent = Agent::new(
            ToolManager::new(),
            Arc::new(MockLlm),
            Arc::new(MemoryHandle::new(
                &data.join("memory").display().to_string(),
            )),
            sessions.clone(),
            Arc::new(WorkspaceMemory::from_paths(root, data)),
        );
        agent.set_session_id("original-thread".into());
        let original_history = vec![Message {
            role: "user".into(),
            content: json!("original history"),
        }];
        agent.history = original_history.clone();
        let mut slot = Some(agent);
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .expect("app");
        app.snapshot.session_id = "original-thread".into();
        app.attach_state_db(db.clone());
        let original = RalphGoal::new("original goal".into(), None);
        app.goal_handle
            .replace(Some(original.clone()))
            .expect("original goal");
        app.goal = Some(original.clone());
        match corrupt {
            "todo" => {
                let path = sessions.todo_file_path("target-thread");
                std::fs::create_dir_all(path.parent().expect("todo parent")).expect("todo dir");
                std::fs::write(path, "{").expect("corrupt todo");
            }
            "runtime" => {
                rusqlite::Connection::open(db.path())
                    .expect("connection")
                    .execute(
                        "UPDATE sessions SET prompt_runtime_json = '{' WHERE id = 'target-thread'",
                        [],
                    )
                    .expect("corrupt runtime JSON");
            }
            other => panic!("unexpected fixture {other}"),
        }
        assert!(
            restore_thread_by_id("target-thread", &mut app, &mut slot)
                .await
                .is_err(),
            "{corrupt}"
        );
        let agent = slot.as_ref().expect("agent retained");
        assert_eq!(agent.session_id, "original-thread", "{corrupt}");
        assert_eq!(agent.history, original_history, "{corrupt}");
        assert_eq!(app.snapshot.session_id, "original-thread", "{corrupt}");
        assert_eq!(app.goal, Some(original.clone()), "{corrupt}");
        assert_eq!(app.goal_handle.snapshot(), Some(original), "{corrupt}");
        app.goal_handle
            .replace(None)
            .expect("clear still addresses original binding");
        assert!(
            db.try_load_goal("original-thread")
                .expect("original row")
                .is_none()
        );
        assert_eq!(
            db.try_load_goal("target-thread").expect("target row"),
            target_row
        );
    }
}

#[tokio::test]
async fn corrupt_goal_does_not_block_requested_or_latest_thread_restore() {
    for (field, value) in [
        ("status", json!("unknown")),
        ("objective", json!("")),
        ("token_budget", json!(0)),
        ("token_budget", json!(u64::from(u32::MAX) + 1)),
        ("tokens_used", json!(u64::from(u32::MAX) + 1)),
        ("turns_completed", json!(u64::from(u32::MAX) + 1)),
        ("tokens_used", json!(-1)),
        ("turns_completed", json!(-1)),
    ] {
        for latest in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = dir.path().join("workspace");
            let data = dir.path().join("state");
            let sessions =
                Arc::new(SessionManager::new_for_rara_dir(data.clone()).expect("sessions"));
            let db = Arc::new(StateDb::new_for_root_dir(data.clone()).expect("state db"));
            seed_thread(&db, "target-thread", &root);
            let target = RalphGoal::new("target goal".into(), Some(1000));
            let mut corrupted = serde_json::to_value(&target).expect("serialize");
            corrupted[field] = value.clone();
            db.save_goal("target-thread", &corrupted)
                .expect("corrupt goal");
            let target_row = db.try_load_goal("target-thread").expect("target row");
            let mut agent = Agent::new(
                ToolManager::new(),
                Arc::new(MockLlm),
                Arc::new(MemoryHandle::new(
                    &data.join("memory").display().to_string(),
                )),
                sessions,
                Arc::new(WorkspaceMemory::from_paths(root, data)),
            );
            agent.set_session_id("original-thread".into());
            agent.history = vec![Message {
                role: "user".into(),
                content: json!("old history"),
            }];
            let mut slot = Some(agent);
            let mut app = TuiApp::new(ConfigManager {
                path: dir.path().join("config.json"),
            })
            .expect("app");
            app.snapshot.session_id = "original-thread".into();
            app.attach_state_db(db.clone());
            let original = RalphGoal::new("original goal".into(), None);
            app.goal_handle
                .replace(Some(original.clone()))
                .expect("original goal");
            app.goal = Some(original);
            let original_row = db.try_load_goal("original-thread").expect("original row");
            if latest {
                restore_latest_thread(&db, &mut app, &mut slot).await
            } else {
                restore_thread_by_id("target-thread", &mut app, &mut slot).await
            }
            .expect("optional goal corruption must not abort thread restore");
            assert_eq!(slot.as_ref().expect("agent").session_id, "target-thread");
            assert!(slot.as_ref().expect("agent").history.is_empty());
            assert_eq!(app.snapshot.session_id, "target-thread");
            assert!(app.goal.is_none());
            assert!(app.goal_handle.snapshot().is_none());
            assert!(
                app.notice_text()
                    .expect("warning")
                    .contains("Goal persistence unavailable")
            );
            assert_eq!(
                app.notice().unwrap().level(),
                crate::tui::state::NoticeLevel::Warning
            );
            let error = app
                .goal_handle
                .replace(Some(RalphGoal::new(
                    "no memory-only replacement".into(),
                    None,
                )))
                .expect_err("goal writes stay disabled");
            assert!(error.to_string().contains("goal persistence unavailable"));
            assert_eq!(
                db.try_load_goal("target-thread")
                    .expect("target row unchanged"),
                target_row
            );
            assert_eq!(
                db.try_load_goal("original-thread")
                    .expect("original row unchanged"),
                original_row
            );
            db.save_goal(
                "target-thread",
                &serde_json::to_value(&target).expect("serialize"),
            )
            .expect("repair fixture");
            restore_thread_by_id("target-thread", &mut app, &mut slot)
                .await
                .expect("valid restore re-enables persistence");
            assert_eq!(app.goal, Some(target));
            app.goal_handle
                .mutate(|goal| {
                    goal.as_mut().expect("goal").status = GoalStatus::Paused;
                    Ok(())
                })
                .expect("durable write re-enabled");
            assert_eq!(
                db.try_load_goal("target-thread")
                    .expect("updated target")
                    .expect("goal")["status"],
                "Paused"
            );
            assert_eq!(
                db.try_load_goal("original-thread")
                    .expect("original row unchanged"),
                original_row
            );
        }
    }
}

fn seed_thread(db: &StateDb, thread_id: &str, root: &std::path::Path) {
    db.upsert_session(
        thread_id,
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
}
