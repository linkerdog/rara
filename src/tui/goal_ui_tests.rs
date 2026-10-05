use crossterm::event::{Event, KeyEvent, KeyModifiers};

use super::*;
use crate::runtime_goals::RalphGoal;
use crate::tui::state::RuntimeSnapshot;
use crate::tui::testing::TuiHarness;

fn harness(status: GoalStatus) -> TuiHarness {
    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let mut goal = RalphGoal::new("finish goal controls".into(), Some(500));
    goal.status = status;
    goal.tokens_used = 125;
    goal.turns_completed = 3;
    tui.app().goal_handle.replace(Some(goal.clone())).unwrap();
    tui.app_mut().goal = Some(goal);
    tui
}

async fn command(tui: &mut TuiHarness, text: &str) {
    tui.app_mut().bottom_pane.input = text.into();
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
}

#[tokio::test]
async fn goal_summary_renders_fields_and_state_appropriate_commands() {
    for (status, action) in [
        (GoalStatus::Pursuing, "/goal pause"),
        (GoalStatus::Paused, "/goal resume"),
        (GoalStatus::Blocked, "/goal resume"),
        (GoalStatus::BudgetLimited, "/goal <objective>"),
        (GoalStatus::Complete, "/goal <objective>"),
    ] {
        let mut tui = harness(status);
        command(&mut tui, "/goal").await;
        let screen = tui.screen_text(100, 30);
        for text in [
            status_label(status),
            "finish goal controls",
            "Time used:",
            "Turns: 3",
            "Tokens: 125",
            "Budget: 500",
            "Remaining: 375 tokens",
            action,
        ] {
            assert!(screen.contains(text), "missing {text}:\n{screen}");
        }
        tui.expect_no_commands();
        tui.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .await
            .unwrap();
        assert!(tui.app().overlay.is_none());
    }
}

#[tokio::test]
async fn goal_editor_preserves_usage_status_and_deferral_with_unicode_cursor() {
    let mut tui = harness(GoalStatus::Paused);
    tui.app().goal_handle.defer_continuation().unwrap();
    command(&mut tui, "/goal edit").await;
    assert_eq!(tui.app().goal_ui.input, "finish goal controls");
    tui.app_mut().goal_ui.input = "👩‍💻e\u{301} end".into();
    tui.app_mut().goal_ui.cursor = Some(5);
    tui.press_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(tui.app().goal_ui.input, "👩‍💻 end");
    tui.send_terminal_event(Event::Paste("updated ".into()))
        .await
        .unwrap();
    let expected = tui.app().goal_ui.input.clone();
    let (screen, cursor) = tui.screen_with_cursor(50, 16);
    assert!(screen.contains("Edit goal objective"));
    let (x, y) = cursor.expect("editor cursor");
    assert!(x < 50 && y < 16);
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    let goal = tui.app().goal_handle.snapshot().unwrap();
    assert_eq!(goal.objective, expected);
    assert_eq!(
        (
            goal.status,
            goal.tokens_used,
            goal.turns_completed,
            goal.token_budget
        ),
        (GoalStatus::Paused, 125, 3, Some(500))
    );
    assert!(tui.app().goal_handle.continuation_deferred());
    tui.expect_no_commands();
}

#[tokio::test]
async fn goal_replacement_requires_acceptance_and_rejects_stale_confirmation() {
    let mut tui = harness(GoalStatus::Pursuing);
    let original = tui.app().goal_handle.snapshot();
    command(&mut tui, "/goal --tokens 20 replacement objective").await;
    let screen = tui.screen_text(100, 30);
    for text in [
        "Replace unfinished goal?",
        "replacement objective",
        "Budget: 20",
        "Replace goal",
        "Keep current goal",
    ] {
        assert!(screen.contains(text), "missing {text}:\n{screen}");
    }
    tui.press_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .await
        .unwrap();
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(tui.app().goal_handle.snapshot(), original);
    command(&mut tui, "/goal replacement objective").await;
    tui.app()
        .goal_handle
        .replace(Some(RalphGoal::new("a different thread goal".into(), None)))
        .unwrap();
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(
        tui.app().goal_handle.snapshot().unwrap().objective,
        "a different thread goal"
    );
    assert!(tui.app().notice_text().unwrap().contains("goal changed"));
    command(&mut tui, "/goal --tokens 20 replacement objective").await;
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    let goal = tui.app().goal_handle.snapshot().unwrap();
    assert_eq!(
        (goal.objective.as_str(), goal.tokens_used, goal.token_budget),
        ("replacement objective", 0, Some(20))
    );
    tui.expect_no_commands();
}

#[tokio::test]
async fn dismissing_or_saving_empty_goal_editor_does_not_mutate_goal() {
    let mut tui = harness(GoalStatus::Blocked);
    let original = tui.app().goal_handle.snapshot();
    command(&mut tui, "/goal edit").await;
    tui.app_mut().goal_ui.input.clear();
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(tui.app().overlay, Some(Overlay::Goal));
    assert_eq!(tui.app().goal_handle.snapshot(), original);
    tui.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(tui.app().goal_handle.snapshot(), original);
    tui.expect_no_commands();
}

#[test]
fn elapsed_invalidation_is_coalesced_and_budget_indicator_does_not_tick() {
    let mut tui = harness(GoalStatus::Pursuing);
    let now = tui.app().goal.as_ref().unwrap().created_at_epoch_seconds;
    assert!(
        !update_elapsed(tui.app_mut(), now),
        "bounded indicator does not display elapsed time"
    );
    tui.app_mut().goal.as_mut().unwrap().token_budget = None;
    assert!(update_elapsed(tui.app_mut(), now));
    assert!(!update_elapsed(tui.app_mut(), now));
    tui.app_mut()
        .goal
        .as_mut()
        .unwrap()
        .created_at_epoch_seconds -= 10;
    assert!(update_elapsed(tui.app_mut(), now));
    assert_eq!(tui.app().goal_ui.displayed_elapsed, Some(10));
    assert!(!update_elapsed(tui.app_mut(), now));
}

#[tokio::test]
async fn goal_pause_remains_available_while_a_turn_finishes() {
    let mut tui = harness(GoalStatus::Pursuing);
    let (_, receiver) = tokio::sync::mpsc::unbounded_channel();
    tui.app_mut().bottom_pane.running_task = Some(crate::tui::state::RunningTask {
        kind: crate::tui::state::TaskKind::Query,
        receiver,
        handle: tokio::spawn(std::future::pending()),
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: None,
    });
    command(&mut tui, "/goal pause").await;
    assert_eq!(
        tui.app().goal_handle.snapshot().unwrap().status,
        GoalStatus::Paused
    );
    assert!(
        tui.app().is_busy(),
        "pause affects the next turn, not the running query"
    );
    let task = tui.app_mut().bottom_pane.running_task.take().unwrap();
    task.handle.abort();
    assert!(matches!(task.handle.await, Err(error) if error.is_cancelled()));
    tui.expect_no_commands();
}
