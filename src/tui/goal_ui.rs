use crossterm::event::KeyCode;

use super::app_event::AppEvent;
use super::state::{Overlay, TuiApp};
use crate::runtime_goals::{GoalResumeTicket, GoalStatus};

#[derive(Default)]
pub(super) struct GoalUiState {
    pub dialog: Option<GoalDialog>,
    pub input: String,
    pub cursor: Option<usize>,
    pub selected: usize,
    pub paused_offer: Option<GoalResumeTicket>,
    pub displayed_elapsed: Option<u64>,
}

pub(super) enum GoalDialog {
    Summary,
    Resume(GoalResumeTicket),
    Edit(GoalResumeTicket),
    Replace {
        ticket: GoalResumeTicket,
        objective: String,
        budget: Option<u32>,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum GoalUiAction {
    Move(i32),
    Accept,
}

pub(super) fn open(app: &mut TuiApp, dialog: GoalDialog) {
    app.goal_ui.selected = 0;
    app.goal_ui.input = app
        .goal
        .as_ref()
        .map(|goal| goal.objective.clone())
        .unwrap_or_default();
    app.goal_ui.cursor = None;
    app.goal_ui.dialog = Some(dialog);
    app.open_overlay(Overlay::Goal);
}

pub(super) fn key_event(app: &TuiApp, code: KeyCode) -> AppEvent {
    match (&app.goal_ui.dialog, code) {
        (_, KeyCode::Esc) => AppEvent::CloseOverlay,
        (Some(GoalDialog::Summary), KeyCode::Enter) => AppEvent::CloseOverlay,
        (_, KeyCode::Enter) => AppEvent::Goal(GoalUiAction::Accept),
        (Some(GoalDialog::Edit(_)), code) => match code {
            KeyCode::Left => AppEvent::MoveCursorLeft,
            KeyCode::Right => AppEvent::MoveCursorRight,
            KeyCode::Home => AppEvent::MoveCursorHome,
            KeyCode::End => AppEvent::MoveCursorEnd,
            KeyCode::Backspace => AppEvent::Backspace,
            KeyCode::Delete => AppEvent::DeleteForward,
            KeyCode::Char(c) => AppEvent::InputChar(c),
            _ => AppEvent::Noop,
        },
        (_, KeyCode::Up | KeyCode::Char('k')) => AppEvent::Goal(GoalUiAction::Move(-1)),
        (_, KeyCode::Down | KeyCode::Char('j')) => AppEvent::Goal(GoalUiAction::Move(1)),
        _ => AppEvent::Noop,
    }
}

pub(super) fn status_label(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Pursuing => "Active",
        GoalStatus::Paused => "Paused",
        GoalStatus::Blocked => "Blocked",
        GoalStatus::Complete => "Complete",
        GoalStatus::BudgetLimited => "BudgetLimited",
    }
}

pub(super) fn valid_commands(app: &TuiApp) -> &'static str {
    match app.goal.as_ref().map(|goal| goal.status) {
        Some(GoalStatus::Pursuing) if app.goal_handle.continuation_deferred() => {
            "/goal resume · /goal pause · /goal edit · /goal clear"
        }
        Some(GoalStatus::Pursuing) => "/goal pause · /goal edit · /goal clear",
        Some(GoalStatus::Paused | GoalStatus::Blocked) => "/goal resume · /goal edit · /goal clear",
        Some(GoalStatus::Complete | GoalStatus::BudgetLimited) => {
            "/goal edit · /goal clear · /goal <objective>"
        }
        None => "/goal <objective> · /goal --tokens <N> <objective>",
    }
}

pub(super) fn update_elapsed(app: &mut TuiApp, now: u64) -> bool {
    let elapsed = app
        .goal
        .as_ref()
        .filter(|goal| goal.token_budget.is_none())
        .map(|goal| now.saturating_sub(goal.created_at_epoch_seconds));
    if app.goal_ui.displayed_elapsed == elapsed {
        return false;
    }
    app.goal_ui.displayed_elapsed = elapsed;
    true
}

#[cfg(test)]
#[path = "goal_ui_tests.rs"]
mod tests;
