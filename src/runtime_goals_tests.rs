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
        ("objective", json!("")),
        ("objective", json!(" \t\n")),
        ("token_budget", json!(0)),
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

#[test]
fn interrupted_goal_marker_survives_database_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = StateDb::new_for_root_dir(dir.path().join("state")).expect("db");
    let mut row =
        serde_json::to_value(RalphGoal::new("keep stopped work idle".into(), None)).unwrap();
    row["continuation_deferred"] = json!(true);
    db.save_goal("interrupted", &row).expect("save");
    assert_eq!(
        db.try_load_goal("interrupted").unwrap().unwrap()["continuation_deferred"],
        true
    );
}

#[test]
fn deferral_survives_mutation_rebuild_and_restart_until_a_new_turn() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let store = GoalStore::default();
    store.restore_for_thread("thread", db.clone()).unwrap();
    store
        .replace(Some(RalphGoal::new("resume safely".into(), None)))
        .unwrap();
    let ticket = store.resume_ticket().unwrap();
    store.defer_continuation().unwrap();
    assert!(
        store
            .claim_continuation(&ticket, super::GoalContinuationMode::Automatic)
            .unwrap()
            .is_none()
    );
    store
        .mutate(|goal| {
            goal.as_mut().unwrap().tokens_used = 7;
            Ok(())
        })
        .unwrap();
    let rebuilt = GoalStore::default();
    rebuilt.inherit_from(&store);
    assert!(rebuilt.continuation_deferred());
    let restored = GoalStore::default();
    restored.restore_for_thread("thread", db.clone()).unwrap();
    assert!(restored.continuation_deferred());
    let ticket = restored.resume_ticket().unwrap();
    restored
        .edit_objective(&ticket, "edited interrupted goal".into())
        .unwrap();
    assert!(
        restored.continuation_deferred(),
        "editing must not restart interrupted work"
    );
    restored.record_turn_started().unwrap();
    assert!(!restored.continuation_deferred());
    assert_eq!(restored.snapshot().unwrap().tokens_used, 7);
    assert_eq!(
        db.try_load_goal("thread").unwrap().unwrap()["continuation_deferred"],
        false
    );
}

#[test]
fn resume_claim_is_single_use_and_rejects_new_turn_or_thread() {
    let store = GoalStore::default();
    store
        .replace(Some(RalphGoal::new("once".into(), None)))
        .unwrap();
    let ticket = store.resume_ticket().unwrap();
    let rebuilt = GoalStore::default();
    rebuilt.inherit_from(&store);
    assert!(rebuilt.matches_resume_ticket(&ticket));
    assert!(
        rebuilt
            .claim_continuation(&ticket, super::GoalContinuationMode::Automatic)
            .unwrap()
            .is_some()
    );
    assert!(
        rebuilt
            .claim_continuation(&ticket, super::GoalContinuationMode::Automatic)
            .unwrap()
            .is_none()
    );
    let ticket = store.resume_ticket().unwrap();
    store.record_turn_started().unwrap();
    assert!(
        store
            .claim_continuation(&ticket, super::GoalContinuationMode::Automatic)
            .unwrap()
            .is_none()
    );
    let ticket = store.resume_ticket().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    store.restore_for_thread("other", db).unwrap();
    assert!(
        store
            .claim_continuation(&ticket, super::GoalContinuationMode::Automatic)
            .unwrap()
            .is_none()
    );
}

#[test]
fn interruption_write_failure_invalidates_queued_work_without_changing_committed_flag() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let store = GoalStore::default();
    store.restore_for_thread("thread", db.clone()).unwrap();
    store
        .replace(Some(RalphGoal::new("stop on error".into(), None)))
        .unwrap();
    let ticket = store.resume_ticket().unwrap();
    let conn = rusqlite::Connection::open(db.path()).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_stop BEFORE INSERT ON goals BEGIN SELECT RAISE(FAIL, 'injected stop failure'); END;").unwrap();
    assert!(store.defer_continuation().is_err());
    assert!(!store.matches_resume_ticket(&ticket));
    assert!(!store.continuation_deferred());
    assert_eq!(
        db.try_load_goal("thread").unwrap().unwrap()["continuation_deferred"],
        false
    );
    conn.execute_batch("DROP TRIGGER reject_stop;").unwrap();
    store.defer_continuation().unwrap();
    conn.execute_batch("CREATE TRIGGER reject_start BEFORE INSERT ON goals BEGIN SELECT RAISE(FAIL, 'injected start failure'); END;").unwrap();
    assert!(store.record_turn_started().is_err());
    assert!(store.continuation_deferred());
    assert_eq!(
        db.try_load_goal("thread").unwrap().unwrap()["continuation_deferred"],
        true
    );
}

#[test]
fn restored_exhausted_budget_requires_a_successful_write_before_admission() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let store = GoalStore::default();
    store.restore_for_thread("thread", db.clone()).unwrap();
    let mut goal = RalphGoal::new("budget wrap-up".into(), Some(10));
    goal.tokens_used = 12;
    store.replace(Some(goal.clone())).unwrap();
    let ticket = store.resume_ticket().unwrap();
    let conn = rusqlite::Connection::open(db.path()).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_budget BEFORE INSERT ON goals BEGIN SELECT RAISE(FAIL, 'injected budget failure'); END;").unwrap();
    assert!(
        store
            .claim_continuation(&ticket, super::GoalContinuationMode::Automatic)
            .is_err()
    );
    assert_eq!(store.snapshot(), Some(goal));
    conn.execute_batch("DROP TRIGGER reject_budget;").unwrap();
    assert_eq!(
        store
            .claim_continuation(&ticket, super::GoalContinuationMode::Automatic)
            .unwrap()
            .unwrap()
            .status,
        GoalStatus::BudgetLimited
    );
    assert_eq!(
        db.try_load_goal("thread").unwrap().unwrap()["status"],
        "BudgetLimited"
    );
    assert!(
        store
            .claim_continuation(&ticket, super::GoalContinuationMode::Automatic)
            .unwrap()
            .is_none()
    );
}

#[test]
fn legacy_goal_table_migrates_and_invalid_deferral_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    let db = StateDb::new_for_root_dir(root.clone()).unwrap();
    let goal = RalphGoal::new("legacy active goal".into(), None);
    db.save_goal("thread", &serde_json::to_value(&goal).unwrap())
        .unwrap();
    let conn = rusqlite::Connection::open(db.path()).unwrap();
    conn.execute_batch("ALTER TABLE goals DROP COLUMN continuation_deferred;")
        .unwrap();
    drop(db);
    let db = Arc::new(StateDb::new_for_root_dir(root).unwrap());
    let store = GoalStore::default();
    assert_eq!(
        store.restore_for_thread("thread", db.clone()).unwrap(),
        Some(goal.clone())
    );
    assert!(!store.continuation_deferred());
    for value in ["2", "-1", "'invalid'"] {
        conn.execute(
            &format!("UPDATE goals SET continuation_deferred = {value}"),
            [],
        )
        .unwrap();
        assert!(store.restore_for_thread("thread", db.clone()).is_err());
        assert_eq!(store.snapshot(), Some(goal.clone()));
    }
}
