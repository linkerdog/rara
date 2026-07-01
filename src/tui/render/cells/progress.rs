use ratatui::text::Line;

use super::super::{
    compact_progress_summary_lines, compact_recent_first_summary_lines, compact_summary_lines,
};
use super::HistoryCell;
use super::summary_cells::{ExploringCell, PlanningCell, RunningCell};
use super::thinking_cells::ThinkingBlockCell;
use crate::tui::state::{ActiveLiveEvent, TranscriptEntry};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProgressRole {
    Thinking,
    Exploring,
    Planning,
    Running,
}

impl ProgressRole {
    pub(super) fn from_entry_role(role: &str) -> Option<Self> {
        match role {
            "Thinking" => Some(Self::Thinking),
            "Exploring" => Some(Self::Exploring),
            "Planning" => Some(Self::Planning),
            "Running" => Some(Self::Running),
            _ => None,
        }
    }

    pub(super) fn from_live_event(event: &ActiveLiveEvent) -> Self {
        match event {
            ActiveLiveEvent::Thinking(_) => Self::Thinking,
            ActiveLiveEvent::ExplorationAction(_) | ActiveLiveEvent::ExplorationNote(_) => {
                Self::Exploring
            }
            ActiveLiveEvent::PlanningAction(_) | ActiveLiveEvent::PlanningNote(_) => Self::Planning,
            ActiveLiveEvent::RunningAction(_) => Self::Running,
        }
    }
}

pub(super) fn progress_entry_message_lines(role: ProgressRole, message: &str) -> Vec<String> {
    match role {
        ProgressRole::Thinking => message
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(ToString::to_string)
            .collect(),
        ProgressRole::Exploring | ProgressRole::Planning | ProgressRole::Running => message
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                line.trim_start_matches("└")
                    .trim_start_matches('•')
                    .trim()
                    .to_string()
            })
            .filter(|line| !line.is_empty())
            .collect(),
    }
}

pub(super) fn explicit_progress_entry_groups<'a>(
    entries: impl Iterator<Item = &'a TranscriptEntry>,
) -> Vec<(ProgressRole, Vec<String>)> {
    let mut groups: Vec<(ProgressRole, Vec<String>)> = Vec::new();
    for entry in entries {
        let Some(role) = ProgressRole::from_entry_role(entry.role.as_str()) else {
            continue;
        };
        let messages = progress_entry_message_lines(role, &entry.message);
        if messages.is_empty() {
            continue;
        }
        if let Some((last_role, last_messages)) = groups.last_mut()
            && *last_role == role
        {
            last_messages.extend(messages);
            continue;
        }
        groups.push((role, messages));
    }
    groups
}

pub(super) fn push_progress_group<'a>(
    cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
    role: ProgressRole,
    messages: Vec<String>,
    active: bool,
    collapsed: bool,
    duration: Option<std::time::Duration>,
) {
    match role {
        ProgressRole::Thinking => cells.push(Box::new(ThinkingBlockCell::new(
            &messages.join("\n"),
            4,
            collapsed,
            duration,
        ))),
        ProgressRole::Exploring => cells.push(Box::new(ExploringCell::new(
            compact_summary_lines(messages.as_slice(), 4, "more exploration step(s)"),
            active,
        ))),
        ProgressRole::Planning => cells.push(Box::new(PlanningCell::new(
            compact_summary_lines(messages.as_slice(), 4, "more planning step(s)"),
            active,
        ))),
        ProgressRole::Running => cells.push(Box::new(RunningCell::new(
            compact_summary_lines(messages.as_slice(), 4, "more running step(s)"),
            active,
        ))),
    }
}

pub(super) fn push_live_events<'a>(
    cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
    events: &[crate::tui::state::ActiveLiveEvent],
    streaming_thinking_lines: Option<&'a [Line<'static>]>,
    active: bool,
    collapsed: bool,
    thinking_duration: Option<std::time::Duration>,
) {
    let mut thinking_messages = Vec::new();
    let mut exploration_actions = Vec::new();
    let mut exploration_notes = Vec::new();
    let mut planning_actions = Vec::new();
    let mut planning_notes = Vec::new();
    let mut running_actions = Vec::new();

    for event in events {
        match ProgressRole::from_live_event(event) {
            ProgressRole::Thinking => {
                push_live_exploration_group(
                    cells,
                    &mut exploration_actions,
                    &mut exploration_notes,
                    active,
                );
                push_live_planning_group(cells, &mut planning_actions, &mut planning_notes, active);
                push_live_running_group(cells, &mut running_actions, active);
                thinking_messages.push(event.message().to_string());
            }
            ProgressRole::Exploring => {
                push_live_thinking_group(
                    cells,
                    &mut thinking_messages,
                    None,
                    collapsed,
                    thinking_duration,
                );
                push_live_planning_group(cells, &mut planning_actions, &mut planning_notes, active);
                push_live_running_group(cells, &mut running_actions, active);
                if event.is_note() {
                    exploration_notes.push(event.message().to_string());
                } else {
                    exploration_actions.push(event.message().to_string());
                }
            }
            ProgressRole::Planning => {
                push_live_thinking_group(
                    cells,
                    &mut thinking_messages,
                    None,
                    collapsed,
                    thinking_duration,
                );
                push_live_exploration_group(
                    cells,
                    &mut exploration_actions,
                    &mut exploration_notes,
                    active,
                );
                push_live_running_group(cells, &mut running_actions, active);
                if event.is_note() {
                    planning_notes.push(event.message().to_string());
                } else {
                    planning_actions.push(event.message().to_string());
                }
            }
            ProgressRole::Running => {
                push_live_thinking_group(
                    cells,
                    &mut thinking_messages,
                    None,
                    collapsed,
                    thinking_duration,
                );
                push_live_exploration_group(
                    cells,
                    &mut exploration_actions,
                    &mut exploration_notes,
                    active,
                );
                push_live_planning_group(cells, &mut planning_actions, &mut planning_notes, active);
                running_actions.push(event.message().to_string());
            }
        }
    }

    push_live_exploration_group(
        cells,
        &mut exploration_actions,
        &mut exploration_notes,
        active,
    );
    push_live_planning_group(cells, &mut planning_actions, &mut planning_notes, active);
    push_live_running_group(cells, &mut running_actions, active);
    push_live_thinking_group(
        cells,
        &mut thinking_messages,
        streaming_thinking_lines,
        collapsed,
        thinking_duration,
    );
}

pub(super) fn push_live_thinking_group<'a>(
    cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
    messages: &mut Vec<String>,
    stream_lines: Option<&'a [Line<'static>]>,
    collapsed: bool,
    duration: Option<std::time::Duration>,
) {
    if messages.is_empty() && stream_lines.is_none_or(|lines| lines.is_empty()) {
        return;
    }
    cells.push(Box::new(ThinkingBlockCell::with_stream_lines(
        std::mem::take(messages).join("\n"),
        stream_lines,
        4,
        collapsed,
        duration,
    )));
}

pub(super) fn push_live_exploration_group<'a>(
    cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
    actions: &mut Vec<String>,
    notes: &mut Vec<String>,
    active: bool,
) {
    if actions.is_empty() && notes.is_empty() {
        return;
    }
    cells.push(Box::new(ExploringCell::new(
        compact_progress_summary_lines(
            actions.as_slice(),
            notes.as_slice(),
            4,
            "more exploration step(s)",
        ),
        active,
    )));
    actions.clear();
    notes.clear();
}

pub(super) fn push_live_planning_group<'a>(
    cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
    actions: &mut Vec<String>,
    notes: &mut Vec<String>,
    active: bool,
) {
    if actions.is_empty() && notes.is_empty() {
        return;
    }
    cells.push(Box::new(PlanningCell::new(
        compact_progress_summary_lines(
            actions.as_slice(),
            notes.as_slice(),
            4,
            "more planning step(s)",
        ),
        active,
    )));
    actions.clear();
    notes.clear();
}

pub(super) fn push_live_running_group<'a>(
    cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
    actions: &mut Vec<String>,
    active: bool,
) {
    if actions.is_empty() {
        return;
    }
    cells.push(Box::new(RunningCell::new(
        compact_recent_first_summary_lines(actions.as_slice(), 4, "more running step(s)"),
        active,
    )));
    actions.clear();
}
