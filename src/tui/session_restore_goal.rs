//! Checked, non-destructive admission of persisted session-local goal snapshots.

use serde_json::Value;

use super::state::{GoalStatus, RalphGoal, TuiApp};

pub(super) fn restore_goal_snapshot(app: &mut TuiApp, thread_id: &str) -> Option<String> {
    let snapshot = app.state_db.as_ref().and_then(|db| db.load_goal(thread_id));
    let (goal, notice) = match snapshot.as_ref().map(decode_snapshot) {
        Some(Ok(goal)) => (goal, None),
        Some(Err(error)) => {
            log::warn!("Goal snapshot for thread {thread_id} was not restored: {error}");
            (
                None,
                Some(format!(
                    "Goal not restored: {error}. Stored snapshot retained."
                )),
            )
        }
        None => (None, None),
    };
    let mut handle = match app.goal_handle.write() {
        Ok(handle) => handle,
        Err(error) => {
            log::warn!("Recovering poisoned goal handle while restoring thread {thread_id}");
            error.into_inner()
        }
    };
    // A failed or absent target snapshot must not retain another thread's goal.
    *handle = goal.clone();
    app.goal_handle.clear_poison();
    app.goal = goal;
    notice
}

fn checked_field(snapshot: &Value, field: &str) -> Result<Option<u32>, String> {
    match snapshot.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .map(Some)
            .ok_or_else(|| format!("{field} must be a nonnegative integer within u32 range")),
    }
}

fn decode_snapshot(snapshot: &Value) -> Result<Option<RalphGoal>, String> {
    let objective = snapshot["objective"].as_str().unwrap_or("");
    if objective.is_empty() {
        return Ok(None);
    }
    let budget = checked_field(snapshot, "token_budget")?;
    let mut goal = RalphGoal::new(objective.to_string(), budget);
    goal.tokens_used = checked_field(snapshot, "tokens_used")?.unwrap_or(0);
    goal.turns_completed = checked_field(snapshot, "turns_completed")?.unwrap_or(0);
    goal.status = match snapshot["status"].as_str() {
        Some("Complete") => GoalStatus::Complete,
        Some("Paused") => GoalStatus::Paused,
        Some("Blocked") => GoalStatus::Blocked,
        Some("BudgetLimited") => GoalStatus::BudgetLimited,
        _ => GoalStatus::Pursuing,
    };
    Ok(Some(goal))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn exact_u32_limits_and_legacy_defaults_are_preserved() {
        let snapshot = json!({"objective": "limit", "status": "Blocked", "token_budget": u32::MAX, "tokens_used": u32::MAX, "turns_completed": u32::MAX});
        let goal = decode_snapshot(&snapshot)
            .expect("valid snapshot")
            .expect("goal");
        assert_eq!(goal.token_budget, Some(u32::MAX));
        assert_eq!(goal.tokens_used, u32::MAX);
        assert_eq!(goal.turns_completed, u32::MAX);
        assert_eq!(goal.status, GoalStatus::Blocked);
        let legacy = decode_snapshot(&json!({"objective": "legacy"}))
            .expect("valid snapshot")
            .expect("goal");
        assert_eq!(
            (
                legacy.token_budget,
                legacy.tokens_used,
                legacy.turns_completed
            ),
            (None, 0, 0)
        );
    }

    #[test]
    fn invalid_numeric_fields_never_become_defaults() {
        for field in ["token_budget", "tokens_used", "turns_completed"] {
            for invalid in [
                json!(u64::from(u32::MAX) + 1),
                json!(u64::MAX),
                json!(-1),
                json!(1.5),
                json!("42"),
            ] {
                let mut snapshot = json!({"objective": "unsafe"});
                snapshot[field] = invalid;
                assert!(
                    decode_snapshot(&snapshot)
                        .expect_err("invalid field")
                        .contains(field)
                );
            }
        }
    }

    #[test]
    fn every_legacy_lifecycle_status_survives_restore() {
        for (status, expected) in [
            ("Pursuing", GoalStatus::Pursuing),
            ("Paused", GoalStatus::Paused),
            ("Blocked", GoalStatus::Blocked),
            ("Complete", GoalStatus::Complete),
            ("BudgetLimited", GoalStatus::BudgetLimited),
        ] {
            let goal = decode_snapshot(
                &json!({"objective": "status", "status": status, "token_budget": null}),
            )
            .expect("valid snapshot")
            .expect("goal");
            assert_eq!(goal.status, expected);
            assert_eq!(goal.token_budget, None);
        }
    }

    #[test]
    fn absent_snapshot_clears_previous_goal_and_recovers_handle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut app = TuiApp::new(crate::config::ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.goal = Some(RalphGoal::new("previous".into(), None));
        let handle = app.goal_handle.clone();
        std::panic::catch_unwind(|| {
            let mut guard = handle.write().expect("goal lock");
            *guard = app.goal.clone();
            panic!("poison fixture");
        })
        .expect_err("poison handle");
        assert!(handle.is_poisoned());
        assert_eq!(restore_goal_snapshot(&mut app, "target"), None);
        assert!(app.goal.is_none());
        assert!(!handle.is_poisoned());
        assert!(handle.read().expect("recovered goal lock").is_none());
    }
}
