use std::sync::Arc;

use rara_state::state_db::StateDb;
use serde_json::json;

use super::{GoalStatus, GoalStore, RalphGoal};

#[test]
fn concurrent_mutations_preserve_every_usage_delta_in_the_durable_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let store = Arc::new(GoalStore::default());
    store
        .restore_for_thread("thread", db.clone())
        .expect("bind");
    store
        .replace(Some(RalphGoal::new("serialize usage".into(), None)))
        .expect("seed goal");
    let workers = (0..4)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || {
                for _ in 0..25 {
                    store
                        .mutate(|stored| {
                            let goal = stored.as_mut().expect("goal");
                            goal.tokens_used += 1;
                            goal.turns_completed += 1;
                            Ok(())
                        })
                        .expect("account usage");
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("writer thread");
    }
    let goal = store.snapshot().expect("committed goal");
    assert_eq!(goal.tokens_used, 100);
    assert_eq!(goal.turns_completed, 100);
    assert_eq!(
        GoalStore::default()
            .restore_for_thread("thread", db)
            .expect("restore"),
        Some(goal)
    );
}

#[test]
fn every_status_usage_and_creation_time_survives_restore_and_clear() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let store = GoalStore::default();
    store
        .restore_for_thread("thread", db.clone())
        .expect("bind");
    for status in [
        GoalStatus::Pursuing,
        GoalStatus::Paused,
        GoalStatus::Blocked,
        GoalStatus::Complete,
        GoalStatus::BudgetLimited,
    ] {
        let mut goal = RalphGoal::new("preserve all fields".into(), Some(1234));
        goal.status = status;
        goal.tokens_used = 234;
        goal.turns_completed = 7;
        goal.created_at_epoch_seconds = 1_234_567_890;
        store.replace(Some(goal.clone())).expect("save");
        let restored = GoalStore::default();
        assert_eq!(
            restored
                .restore_for_thread("thread", db.clone())
                .expect("restore"),
            Some(goal)
        );
    }
    store.replace(None).expect("clear");
    assert!(db.try_load_goal("thread").expect("query").is_none());
    assert!(
        GoalStore::default()
            .restore_for_thread("thread", db)
            .expect("restore clear")
            .is_none()
    );
}

#[test]
fn replacement_uses_new_creation_time_and_rebuild_preserves_binding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let original = GoalStore::default();
    original
        .restore_for_thread("thread", db.clone())
        .expect("bind");
    let mut goal = RalphGoal::new("old goal".into(), None);
    goal.status = GoalStatus::Complete;
    goal.created_at_epoch_seconds = 10;
    original.replace(Some(goal)).expect("old goal");
    let rebuilt = GoalStore::default();
    rebuilt.inherit_from(&original);
    let mut replacement = RalphGoal::new("new goal".into(), None);
    replacement.created_at_epoch_seconds = 20;
    rebuilt.replace(Some(replacement.clone())).expect("replace");
    assert_eq!(
        GoalStore::default()
            .restore_for_thread("thread", db)
            .expect("restore"),
        Some(replacement)
    );
}

#[test]
fn corrupt_goal_never_becomes_active_or_overwrites_the_current_binding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let store = GoalStore::default();
    store.restore_for_thread("valid", db.clone()).expect("bind");
    let goal = RalphGoal::new("valid goal".into(), None);
    store.replace(Some(goal.clone())).expect("valid goal");
    for (field, value) in [
        ("status", json!("Unknown")),
        ("tokens_used", json!(u64::from(u32::MAX) + 1)),
        ("turns_completed", json!(-1)),
        ("token_budget", json!(u64::from(u32::MAX) + 1)),
    ] {
        let mut corrupt = serde_json::to_value(&goal).expect("serialize");
        corrupt[field] = value;
        db.save_goal("corrupt", &corrupt).expect("seed corrupt row");
        assert!(
            store.restore_for_thread("corrupt", db.clone()).is_err(),
            "{field}"
        );
        assert_eq!(store.snapshot(), Some(goal.clone()));
    }
    db.save_goal("corrupt", &serde_json::to_value(&goal).expect("serialize"))
        .expect("seed timestamp row");
    rusqlite::Connection::open(db.path())
        .expect("independent connection")
        .execute(
            "UPDATE goals SET created_at = -1 WHERE session_id = 'corrupt'",
            [],
        )
        .expect("seed invalid timestamp");
    assert!(store.restore_for_thread("corrupt", db.clone()).is_err());
    assert_eq!(store.snapshot(), Some(goal));
    store
        .replace(None)
        .expect("clear still addresses valid thread");
    assert!(db.try_load_goal("valid").expect("valid query").is_none());
    assert!(
        db.try_load_goal("corrupt")
            .expect("corrupt query")
            .is_some()
    );
}

#[test]
fn write_failure_does_not_publish_a_new_goal_or_clear_the_old_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).expect("state db"));
    let store = GoalStore::default();
    store
        .restore_for_thread("thread", db.clone())
        .expect("bind");
    let mut goal = RalphGoal::new("keep committed snapshot".into(), None);
    goal.created_at_epoch_seconds = 42;
    store.replace(Some(goal.clone())).expect("save");
    let connection = rusqlite::Connection::open(db.path()).expect("independent connection");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_goal_write BEFORE INSERT ON goals
         BEGIN SELECT RAISE(FAIL, 'injected goal write failure'); END;
         CREATE TRIGGER reject_goal_delete BEFORE DELETE ON goals
         BEGIN SELECT RAISE(FAIL, 'injected goal delete failure'); END;",
        )
        .expect("failure triggers");
    let error = store
        .mutate(|next| {
            next.as_mut().expect("goal").status = GoalStatus::Paused;
            Ok(())
        })
        .expect_err("SQLite write must fail");
    assert!(error.to_string().contains("injected goal write failure"));
    assert_eq!(store.snapshot(), Some(goal.clone()));
    assert!(store.replace(None).is_err());
    assert_eq!(store.snapshot(), Some(goal.clone()));
    assert_eq!(
        GoalStore::default()
            .restore_for_thread("thread", db)
            .expect("durable snapshot"),
        Some(goal)
    );
}
