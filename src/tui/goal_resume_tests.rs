use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::runtime_goals::RalphGoal;
use crate::tui::state::{Overlay, PermissionMode, RuntimeSnapshot};
use crate::tui::testing::TuiHarness;

fn harness(status: GoalStatus) -> TuiHarness {
    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let mut goal = RalphGoal::new("finish the reviewed work".into(), None);
    goal.status = status;
    tui.app_mut()
        .goal_handle
        .replace(Some(goal.clone()))
        .unwrap();
    tui.app_mut().goal = Some(goal);
    arm_after_restore(tui.app_mut());
    tui
}

#[tokio::test]
async fn resume_queues_exactly_once_after_readiness_and_overlay_gates() {
    let mut tui = harness(GoalStatus::Pursuing);
    tui.queue_restored_goal(AgentReadiness::Unavailable).await;
    tui.expect_no_commands();
    tui.app_mut().open_overlay(Overlay::Context);
    tui.queue_restored_goal(AgentReadiness::Ready).await;
    tui.expect_no_commands();
    tui.app_mut().dismiss_overlay();
    tui.app_mut().agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    tui.queue_restored_goal(AgentReadiness::Ready).await;
    tui.expect_no_commands();
    tui.app_mut().agent_execution_mode = crate::agent::AgentExecutionMode::Execute;
    tui.app_mut().permission_mode = PermissionMode::Auto;
    let ticket = tui.app().goal_handle.resume_ticket().unwrap();
    for _ in 0..5 {
        tui.queue_restored_goal(AgentReadiness::Ready).await;
    }
    tui.expect_command(RuntimeCommand::ContinueGoal {
        ticket,
        mode: GoalContinuationMode::Automatic,
    });
    assert_eq!(tui.app().permission_mode, PermissionMode::Auto);
}

#[tokio::test]
async fn inactive_cleared_and_interrupted_goals_never_queue() {
    for status in [
        GoalStatus::Paused,
        GoalStatus::Blocked,
        GoalStatus::Complete,
        GoalStatus::BudgetLimited,
    ] {
        let mut tui = harness(status);
        tui.queue_restored_goal(AgentReadiness::Ready).await;
        tui.expect_no_commands();
        if status == GoalStatus::Paused {
            let screen = tui.screen_text(100, 30);
            assert!(screen.contains("Resume paused goal?"));
            assert!(screen.contains("Resume goal"));
            assert!(screen.contains("Leave paused"));
            tui.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                .await
                .unwrap();
            tui.queue_restored_goal(AgentReadiness::Ready).await;
            assert!(tui.app().overlay.is_none());
            assert_eq!(
                tui.app().goal_handle.snapshot().unwrap().status,
                GoalStatus::Paused
            );
        }
    }
    let mut tui = harness(GoalStatus::Pursuing);
    tui.app().goal_handle.defer_continuation().unwrap();
    arm_after_restore(tui.app_mut());
    tui.queue_restored_goal(AgentReadiness::Ready).await;
    tui.expect_no_commands();
    assert!(tui.screen_text(100, 30).contains("interrupted"));
    tui.app().goal_handle.replace(None).unwrap();
    tui.app_mut().goal = None;
    arm_after_restore(tui.app_mut());
    tui.queue_restored_goal(AgentReadiness::Ready).await;
    tui.expect_no_commands();
}

#[tokio::test]
async fn new_turn_replacement_and_queued_user_input_take_precedence() {
    for invalidate in ["turn", "replace", "clear"] {
        let mut tui = harness(GoalStatus::Pursuing);
        match invalidate {
            "turn" => tui.app().goal_handle.record_turn_started().unwrap(),
            "replace" => tui
                .app()
                .goal_handle
                .replace(Some(RalphGoal::new("other".into(), None)))
                .unwrap(),
            "clear" => tui.app().goal_handle.replace(None).unwrap(),
            _ => unreachable!(),
        }
        tui.queue_restored_goal(AgentReadiness::Ready).await;
        tui.expect_no_commands();
    }
    let mut tui = harness(GoalStatus::Pursuing);
    tui.app_mut()
        .bottom_pane
        .queued_follow_up_messages
        .push("user work first".into());
    tui.queue_restored_goal(AgentReadiness::Ready).await;
    tui.expect_no_commands();
    tui.app_mut().bottom_pane.queued_follow_up_messages.clear();
    tui.app_mut().show_pending_plan_approval(None);
    tui.queue_restored_goal(AgentReadiness::Ready).await;
    tui.expect_no_commands();
}

#[tokio::test]
async fn terminal_goal_deferral_does_not_offer_an_invalid_resume_command() {
    for status in [GoalStatus::Complete, GoalStatus::BudgetLimited] {
        let mut tui = harness(status);
        tui.app().goal_handle.defer_continuation().unwrap();
        arm_after_restore(tui.app_mut());
        tui.queue_restored_goal(AgentReadiness::Ready).await;
        tui.expect_no_commands();
        assert!(tui.app().overlay.is_none());
        assert!(tui.app().notice_text().is_none());
    }
}
