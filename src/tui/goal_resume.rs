use super::runtime_port::{RuntimeClientPort, RuntimeCommand};
use super::state::TuiApp;
use crate::runtime_goals::{GoalContinuationMode, GoalResumeTicket, GoalStatus};

#[derive(Clone)]
pub(super) struct PendingGoalResume {
    pub ticket: GoalResumeTicket,
    pub enqueued: bool,
    pub mode: GoalContinuationMode,
}

#[derive(Clone, Copy)]
pub(super) enum AgentReadiness {
    Ready,
    Unavailable,
}

pub(super) fn arm_after_restore(app: &mut TuiApp) {
    app.pending_goal_resume = None;
    app.goal_ui.paused_offer = None;
    if app
        .goal
        .as_ref()
        .is_some_and(|goal| goal.status == GoalStatus::Paused)
    {
        app.goal_ui.paused_offer = app.goal_handle.resume_ticket();
    }
    if app.goal_handle.continuation_deferred() {
        if app
            .goal
            .as_ref()
            .is_some_and(|goal| goal.status == GoalStatus::Pursuing)
        {
            app.push_notice(
                "Goal continuation was interrupted. Send a new prompt or use /goal resume to continue.",
            );
        }
        return;
    }
    if app
        .goal
        .as_ref()
        .is_some_and(|goal| goal.status == GoalStatus::Pursuing)
    {
        app.pending_goal_resume = app
            .goal_handle
            .resume_ticket()
            .map(|ticket| PendingGoalResume {
                ticket,
                enqueued: false,
                mode: GoalContinuationMode::Automatic,
            });
    }
}

pub(super) fn idle_for_goal(app: &TuiApp, mode: GoalContinuationMode) -> bool {
    !app.is_busy()
        && app.active_pending_interaction().is_none()
        && app.overlay.is_none()
        && (mode == GoalContinuationMode::Requested
            || app.agent_execution_mode == crate::agent::AgentExecutionMode::Execute)
        && app.bottom_pane.pending_follow_up_messages.is_empty()
        && app.bottom_pane.queued_follow_up_messages.is_empty()
        && !app.bottom_pane.has_pending_planning_suggestion()
}

pub(super) async fn queue_if_idle(
    app: &mut TuiApp,
    port: &dyn RuntimeClientPort,
    readiness: AgentReadiness,
) -> bool {
    if let Some(ticket) = app.goal_ui.paused_offer.as_ref() {
        if !app.goal_handle.matches_resume_ticket(ticket) {
            app.goal_ui.paused_offer = None;
        } else if matches!(readiness, AgentReadiness::Ready)
            && !app.is_busy()
            && app.active_pending_interaction().is_none()
            && app.overlay.is_none()
            && let Some(ticket) = app.goal_ui.paused_offer.take()
        {
            super::goal_ui::open(app, super::goal_ui::GoalDialog::Resume(ticket));
            return true;
        }
    }
    let Some(pending) = app.pending_goal_resume.as_ref() else {
        return false;
    };
    if !app.goal_handle.matches_resume_ticket(&pending.ticket) {
        app.pending_goal_resume = None;
        return false;
    }
    if pending.enqueued
        || matches!(readiness, AgentReadiness::Unavailable)
        || !idle_for_goal(app, pending.mode)
    {
        return false;
    }
    let ticket = pending.ticket.clone();
    let mode = pending.mode;
    match port
        .send(RuntimeCommand::ContinueGoal { ticket, mode })
        .await
    {
        Ok(()) => {
            if let Some(pending) = app.pending_goal_resume.as_mut() {
                pending.enqueued = true;
            }
        }
        Err(error) => {
            app.pending_goal_resume = None;
            log::warn!("Failed to queue restored goal: {error:#}");
            app.push_notice(format!(
                "Goal resume failed: {error:#}. Use /goal resume to retry."
            ));
        }
    }
    true
}

pub(super) fn record_turn_started(app: &mut TuiApp) {
    app.pending_goal_resume = None;
    if let Err(error) = app.goal_handle.record_turn_started() {
        log::warn!("Failed to clear goal interruption deferral: {error:#}");
        app.push_notice(format!("Goal continuation remains deferred: {error:#}"));
    }
}

pub(super) fn defer_for_user_stop(app: &mut TuiApp) {
    app.pending_goal_resume = None;
    if let Err(error) = app.goal_handle.defer_continuation() {
        log::warn!("Failed to persist goal interruption: {error:#}");
        app.push_notice(format!(
            "Could not save goal interruption: {error:#}. This goal may resume after restart."
        ));
    }
}

#[cfg(test)]
#[path = "goal_resume_tests.rs"]
mod tests;
