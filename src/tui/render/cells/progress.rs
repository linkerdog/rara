use ratatui::text::Line;

use super::super::compact_summary_lines;
use super::HistoryCell;
use super::summary_cells::{ExploringCell, PlanningCell, RunningCell};
use super::thinking_cells::ThinkingBlockCell;
use crate::tui::state::TranscriptEntry;

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

pub(super) fn push_streaming_thinking<'a>(
    cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
    streaming_thinking_lines: Option<&'a [Line<'static>]>,
    collapsed: bool,
    thinking_duration: Option<std::time::Duration>,
) {
    let Some(stream_lines) = streaming_thinking_lines.filter(|lines| !lines.is_empty()) else {
        return;
    };
    cells.push(Box::new(ThinkingBlockCell::with_stream_lines(
        String::new(),
        Some(stream_lines),
        4,
        collapsed,
        thinking_duration,
    )));
}
