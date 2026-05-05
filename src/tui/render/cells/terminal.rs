

use super::components::TerminalCell;
use super::TerminalCellData;
use crate::tui::state::{TranscriptEntry, TranscriptEntryPayload};
use crate::tui::terminal_event::{
    TerminalCollectionEvent, TerminalCommandEvent, TerminalEvent, TerminalTarget,
};

pub(super) fn terminal_cell_from_entries<'a>(
    entries: impl DoubleEndedIterator<Item = &'a TranscriptEntry>,
) -> Option<TerminalCell> {
    let data = entries
        .filter_map(terminal_cell_data_from_entry)
        .next_back()?;
    Some(TerminalCell::new(
        data.command,
        data.output,
        data.active,
        data.success,
    ))
}

pub(super) fn terminal_cell_data_from_entry(entry: &TranscriptEntry) -> Option<TerminalCellData> {
    if let Some(TranscriptEntryPayload::Terminal(event)) = entry.payload.as_ref() {
        return terminal_cell_data_from_event(&event);
    }

    if matches!(entry.role.as_str(), "Tool Result" | "Tool Error") {
        return parse_terminal_tool_result(&entry.message);
    }

    None
}

pub(super) fn terminal_cell_data_from_event(event: &TerminalEvent) -> Option<TerminalCellData> {
    match event {
        TerminalEvent::Begin(command) => Some(terminal_cell_data_from_command(command, true)),
        TerminalEvent::End(command) => Some(terminal_cell_data_from_command(command, false)),
        TerminalEvent::List(collection) => {
            Some(terminal_cell_data_from_collection(collection, "list"))
        }
        TerminalEvent::Stop(collection) => {
            Some(terminal_cell_data_from_collection(collection, "stop"))
        }
        TerminalEvent::OutputDelta(_) => None,
    }
}

pub(super) fn terminal_cell_data_from_command(
    command: &TerminalCommandEvent,
    force_active: bool,
) -> TerminalCellData {
    let target = match command.target {
        TerminalTarget::Pty => "pty",
        TerminalTarget::BackgroundTask => "background",
    };
    let command_label = command
        .command
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(command.id.as_deref())
        .unwrap_or("command");
    let mut output = command.output.clone();
    if let Some(output_path) = command.output_path.as_deref() {
        if !output_path.trim().is_empty() {
            output.push(output_path.to_string());
        }
    }
    TerminalCellData {
        command: format!("{target} {command_label}"),
        output,
        active: force_active || command.status == "running",
        success: if force_active {
            None
        } else {
            terminal_status_success(&command.status)
        },
    }
}

pub(super) fn terminal_cell_data_from_collection(
    collection: &TerminalCollectionEvent,
    action: &str,
) -> TerminalCellData {
    let target = match collection.target {
        TerminalTarget::Pty => "pty",
        TerminalTarget::BackgroundTask => "background",
    };
    let output = collection
        .items
        .iter()
        .take(6)
        .map(|item| {
            let id = item.id.as_deref().unwrap_or("<unknown>");
            let command = item
                .command
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("command unavailable");
            format!("{id} {}: {command}", item.status)
        })
        .collect::<Vec<_>>();
    TerminalCellData {
        command: format!("{target} {action}"),
        output,
        active: false,
        success: Some(!collection.items.iter().any(|item| item.is_error)),
    }
}

pub(super) fn parse_terminal_tool_result(message: &str) -> Option<TerminalCellData> {
    let mut lines = message.lines();
    let first = lines.next()?.trim();

    let mut output = Vec::new();
    let mut in_output = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "output:" {
            in_output = true;
            continue;
        }
        if let Some(inline_output) = trimmed.strip_prefix("output:") {
            let inline_output = inline_output.trim();
            if !inline_output.is_empty() {
                output.push(inline_output.to_string());
            }
            in_output = true;
            continue;
        }
        if in_output {
            output.push(line.trim_end().to_string());
        }
    }

    if let Some(rest) = first.strip_prefix("pty ") {
        let (head, command) = parse_terminal_result_head(rest);
        let status = head.split_whitespace().nth(1).unwrap_or("unknown");
        return Some(TerminalCellData {
            command: format!("pty {command}"),
            output,
            active: status == "running",
            success: terminal_status_success(status),
        });
    }

    if let Some(rest) = first.strip_prefix("background task ") {
        let (head, command) = parse_terminal_result_head(rest);
        let status = head.split_whitespace().nth(1).unwrap_or("unknown");
        return Some(TerminalCellData {
            command: format!("background {command}"),
            output,
            active: status == "running",
            success: terminal_status_success(status),
        });
    }

    None
}

pub(super) fn parse_terminal_result_head(rest: &str) -> (&str, String) {
    if let Some((head, command)) = rest.split_once(": ") {
        return (head, command.to_string());
    }
    let fallback = rest
        .split_whitespace()
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(rest);
    (rest, fallback.to_string())
}

pub(super) fn terminal_status_success(status: &str) -> Option<bool> {
    match status {
        "running" => None,
        "completed" | "stopped" => Some(true),
        "failed" | "killed" => Some(false),
        _ => None,
    }
}
