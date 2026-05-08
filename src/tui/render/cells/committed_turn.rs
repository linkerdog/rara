use std::path::Path;

use ratatui::text::Line;

use super::super::history_pipeline::{narrative_entries, ordered_completion_entries};
use super::interaction_cells::CommittedInteractionCell;
use super::message_cell::MessageCell;
use super::progress::{
    ProgressRole, explicit_progress_entry_groups, progress_entry_message_lines, push_progress_group,
};
use super::summary_cells::{ExploredCell, RanCell};
use super::terminal::terminal_cell_from_entries;
use super::user_startup::UserCell;
use super::{
    HistoryCell, InteractionCompletionKind, is_progress_stack_title, trim_trailing_empty_lines,
};
use crate::tui::render::{
    current_turn_exploration_summary_from_entries, current_turn_tool_summary,
};
use crate::tui::state::{TranscriptEntry, TranscriptEntryPayload};
pub(crate) struct CommittedTurnCell<'a> {
    entries: &'a [TranscriptEntry],
    cwd: Option<&'a Path>,
}

impl<'a> CommittedTurnCell<'a> {
    pub(crate) fn new(entries: &'a [TranscriptEntry], cwd: Option<&'a Path>) -> Self {
        Self { entries, cwd }
    }
}

impl HistoryCell for CommittedTurnCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut cells: Vec<Box<dyn HistoryCell + '_>> = Vec::new();
        if let Some(user) = self.entries.iter().find(|entry| entry.role == "You") {
            cells.push(Box::new(UserCell::new(user.message.clone())));
        }

        let entry_refs = self.entries.iter().collect::<Vec<_>>();
        let explicit_progress_groups = explicit_progress_entry_groups(self.entries.iter());
        let preserve_interleaved_agent_order = self
            .entries
            .iter()
            .filter(|entry| entry.role == "Agent")
            .count()
            > 1
            && !explicit_progress_groups.is_empty();
        let has_tool_activity = entry_refs.iter().any(|entry| {
            matches!(
                entry.role.as_str(),
                "Tool" | "Tool Result" | "Tool Error" | "Tool Progress"
            ) || matches!(entry.payload, Some(TranscriptEntryPayload::Terminal(_)))
        });
        if preserve_interleaved_agent_order {
            push_ordered_committed_activity(&mut cells, self.entries, self.cwd);
        } else if explicit_progress_groups.is_empty() {
            if let Some(summary) =
                current_turn_exploration_summary_from_entries(entry_refs.as_slice(), false, None)
            {
                cells.push(Box::new(ExploredCell::new(summary)));
            }

            if let Some(cell) = terminal_cell_from_entries(self.entries.iter()) {
                cells.push(Box::new(cell));
            } else if let Some(summary) =
                current_turn_tool_summary(entry_refs.as_slice(), false, None)
            {
                cells.push(Box::new(RanCell::new(summary)));
            }
        } else {
            for (role, messages) in explicit_progress_groups {
                push_progress_group(&mut cells, role, messages, false);
            }
            if let Some(cell) = terminal_cell_from_entries(self.entries.iter()) {
                cells.push(Box::new(cell));
            }
        }

        if !preserve_interleaved_agent_order {
            let completion_entries = ordered_completion_entries(self.entries);
            let narrative_entries = narrative_entries(
                self.entries,
                has_tool_activity,
                super::is_renderable_system_message,
            );

            for entry in completion_entries {
                let kind = match entry.kind {
                    super::super::history_pipeline::CommittedCompletionKind::ShellApprovalCompleted => {
                        InteractionCompletionKind::ShellApprovalCompleted
                    }
                    super::super::history_pipeline::CommittedCompletionKind::PlanningQuestionAnswered => {
                        InteractionCompletionKind::PlanningQuestionAnswered
                    }
                    super::super::history_pipeline::CommittedCompletionKind::ExplorationQuestionAnswered => {
                        InteractionCompletionKind::ExplorationQuestionAnswered
                    }
                    super::super::history_pipeline::CommittedCompletionKind::SubAgentQuestionAnswered => {
                        InteractionCompletionKind::SubAgentQuestionAnswered
                    }
                    super::super::history_pipeline::CommittedCompletionKind::QuestionAnswered => {
                        InteractionCompletionKind::QuestionAnswered
                    }
                };
                cells.push(Box::new(CommittedInteractionCell::new(kind, entry.message)));
            }

            for entry in narrative_entries {
                let max_lines = if entry.role == "Agent" { usize::MAX } else { 4 };
                cells.push(Box::new(MessageCell::new(
                    &entry.role,
                    &entry.message,
                    max_lines,
                    self.cwd,
                )));
            }
        }

        let mut lines = Vec::new();
        let mut previous_was_progress_stack_title = false;
        for (idx, cell) in cells.into_iter().enumerate() {
            let cell_lines = cell.display_lines(width);
            let current_is_progress_stack_title =
                cell_lines.first().is_some_and(is_progress_stack_title);
            if idx > 0 && !(previous_was_progress_stack_title && current_is_progress_stack_title) {
                lines.push(Line::from(""));
            }
            lines.extend(cell_lines);
            previous_was_progress_stack_title = current_is_progress_stack_title;
        }

        trim_trailing_empty_lines(&mut lines);
        trim_trailing_empty_lines(&mut lines);
        lines
    }
}

fn push_ordered_committed_activity<'a>(
    cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
    entries: &'a [TranscriptEntry],
    cwd: Option<&'a Path>,
) {
    let mut pending_progress: Option<(ProgressRole, Vec<String>)> = None;

    let flush_progress =
        |cells: &mut Vec<Box<dyn HistoryCell + 'a>>,
         pending_progress: &mut Option<(ProgressRole, Vec<String>)>| {
            if let Some((role, messages)) = pending_progress.take() {
                push_progress_group(cells, role, messages, false);
            }
        };

    for entry in entries {
        if entry.role == "You" {
            continue;
        }

        if let Some(role) = ProgressRole::from_entry_role(entry.role.as_str()) {
            let messages = progress_entry_message_lines(role, &entry.message);
            if messages.is_empty() {
                continue;
            }
            if let Some((last_role, last_messages)) = pending_progress.as_mut()
                && *last_role == role
            {
                last_messages.extend(messages);
            } else {
                flush_progress(cells, &mut pending_progress);
                pending_progress = Some((role, messages));
            }
            continue;
        }

        flush_progress(cells, &mut pending_progress);

        if let Some(cell) = terminal_cell_from_entries(std::iter::once(entry)) {
            cells.push(Box::new(cell));
            continue;
        }

        if let Some(kind) = InteractionCompletionKind::from_role(entry.role.as_str()) {
            cells.push(Box::new(CommittedInteractionCell::new(
                kind,
                entry.message.clone(),
            )));
            continue;
        }

        if entry.role == "Agent"
            || (entry.role == "System" && super::is_renderable_system_message(&entry.message))
        {
            let max_lines = if entry.role == "Agent" { usize::MAX } else { 4 };
            cells.push(Box::new(MessageCell::new(
                &entry.role,
                &entry.message,
                max_lines,
                cwd,
            )));
        }
    }

    flush_progress(cells, &mut pending_progress);
}
