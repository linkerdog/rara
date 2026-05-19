use std::path::Path;

use ratatui::text::Line;

use super::interaction_cells::CommittedInteractionCell;
use super::message_cell::MessageCell;
use super::progress::{ProgressRole, progress_entry_message_lines, push_progress_group};
use super::terminal::terminal_cell_from_entries;
use super::user_startup::UserCell;
use super::{
    HistoryCell, InteractionCompletionKind, LspDiagnosticsCell, is_progress_stack_title,
    trim_trailing_empty_lines,
};
use crate::tui::state::{TranscriptEntry, TranscriptEntryPayload};

const TOOL_MESSAGE_MAX_LINES: usize = 5;

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
        push_ordered_committed_activity(&mut cells, self.entries, self.cwd);

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
            flush_progress(cells, &mut pending_progress);
            cells.push(Box::new(UserCell::new(entry.message.clone())));
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

        if matches!(
            entry.role.as_str(),
            "Tool" | "Tool Result" | "Tool Error" | "Tool Progress"
        ) {
            if matches!(entry.role.as_str(), "Tool Result" | "Tool Error")
                && let Some(cell) = LspDiagnosticsCell::from_message(&entry.message)
            {
                cells.push(Box::new(cell));
                continue;
            }
            cells.push(Box::new(MessageCell::new_tail(
                &entry.role,
                &entry.message,
                TOOL_MESSAGE_MAX_LINES,
                cwd,
            )));
            continue;
        }

        if matches!(
            entry.payload,
            Some(TranscriptEntryPayload::Terminal(
                crate::tui::terminal_event::TerminalEvent::OutputDelta(_)
            ))
        ) {
            continue;
        }

        if entry.role == "Agent"
            || (entry.role == "System" && super::is_renderable_system_message(entry))
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
