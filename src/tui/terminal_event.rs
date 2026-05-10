use std::collections::VecDeque;

use rara_tools::tool::ToolOutputStream;
use serde::{Deserialize, Serialize};

use crate::tools::bash::BashCommandInput;
use crate::tui::tool_text::compact_instruction;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalTarget {
    Pty,
    BackgroundTask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalStream {
    Stdout,
    Stderr,
}

impl From<ToolOutputStream> for TerminalStream {
    fn from(stream: ToolOutputStream) -> Self {
        match stream {
            ToolOutputStream::Stdout => Self::Stdout,
            ToolOutputStream::Stderr => Self::Stderr,
        }
    }
}

impl From<TerminalStream> for ToolOutputStream {
    fn from(stream: TerminalStream) -> Self {
        match stream {
            TerminalStream::Stdout => Self::Stdout,
            TerminalStream::Stderr => Self::Stderr,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TerminalEvent {
    Begin(TerminalCommandEvent),
    OutputDelta(TerminalOutputDeltaEvent),
    End(TerminalCommandEvent),
    List(TerminalCollectionEvent),
    Stop(TerminalCollectionEvent),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalCommandEvent {
    pub target: TerminalTarget,
    pub id: Option<String>,
    pub status: String,
    pub command: Option<String>,
    pub exit_code: Option<i64>,
    pub output: Vec<String>,
    pub output_path: Option<String>,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalOutputDeltaEvent {
    pub target: TerminalTarget,
    pub id: Option<String>,
    pub stream: TerminalStream,
    pub chunk: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalCollectionEvent {
    pub target: TerminalTarget,
    pub items: Vec<TerminalCommandEvent>,
}

impl TerminalEvent {
    pub(crate) fn from_tool_use(name: &str, input: &serde_json::Value) -> Option<Self> {
        match name {
            "pty_start" => {
                let command = input
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .map(compact_instruction);
                Some(Self::Begin(TerminalCommandEvent::new(
                    TerminalTarget::Pty,
                    None,
                    "starting",
                    command,
                )))
            }
            "bash" => {
                let request = BashCommandInput::from_value(input.clone()).ok()?;
                if !request.run_in_background {
                    return None;
                }
                Some(Self::Begin(TerminalCommandEvent::new(
                    TerminalTarget::BackgroundTask,
                    None,
                    "starting",
                    Some(request.summary()),
                )))
            }
            _ => None,
        }
    }

    pub(crate) fn from_tool_progress(
        name: &str,
        stream: ToolOutputStream,
        chunk: &str,
    ) -> Option<Self> {
        let target = match name {
            "pty_start" | "pty_read" | "pty_status" | "pty_write" | "pty_kill" => {
                TerminalTarget::Pty
            }
            "bash" | "background_task_status" => TerminalTarget::BackgroundTask,
            _ => return None,
        };
        let chunk = output_tail_preview(chunk).map(|lines| lines.join("\n"))?;
        Some(Self::OutputDelta(TerminalOutputDeltaEvent {
            target,
            id: None,
            stream: stream.into(),
            chunk,
        }))
    }

    pub(crate) fn from_tool_result(name: &str, content: &str, is_error: bool) -> Option<Self> {
        let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
        match name {
            "bash" => background_bash_start_event(&value, is_error).map(Self::End),
            "pty_start" | "pty_read" | "pty_status" | "pty_write" | "pty_kill" => {
                Some(Self::End(pty_command_event(&value, is_error)))
            }
            "pty_list" => Some(Self::List(collection_event(
                TerminalTarget::Pty,
                value.get("sessions"),
                is_error,
            ))),
            "pty_stop" => Some(Self::Stop(collection_event(
                TerminalTarget::Pty,
                value.get("stopped"),
                is_error,
            ))),
            "background_task_status" => Some(Self::End(background_task_event(&value, is_error))),
            "background_task_list" => Some(Self::List(collection_event(
                TerminalTarget::BackgroundTask,
                value.get("tasks"),
                is_error,
            ))),
            "background_task_stop" => Some(Self::Stop(collection_event(
                TerminalTarget::BackgroundTask,
                value.get("stopped"),
                is_error,
            ))),
            _ => None,
        }
    }

    pub(crate) fn transcript_role(&self) -> &'static str {
        match self {
            Self::Begin(_) | Self::OutputDelta(_) => "Tool",
            Self::End(event) if event.is_error => "Tool Error",
            Self::List(event) | Self::Stop(event)
                if event.items.iter().any(|item| item.is_error) =>
            {
                "Tool Error"
            }
            Self::End(_) | Self::List(_) | Self::Stop(_) => "Tool Result",
        }
    }

    pub(crate) fn to_transcript_message(&self) -> String {
        match self {
            Self::Begin(event) => {
                let command = event
                    .command
                    .as_deref()
                    .filter(|command| !command.is_empty())
                    .unwrap_or("command");
                format!("{} {}", event.tool_name(), command)
            }
            Self::OutputDelta(event) => {
                let stream = match event.stream {
                    TerminalStream::Stdout => "stdout",
                    TerminalStream::Stderr => "stderr",
                };
                format!(
                    "{} {stream}:\n{}",
                    event.tool_name(),
                    event.chunk.trim_end()
                )
            }
            Self::End(event) => event.to_result_message(),
            Self::List(event) => event.to_collection_message("list"),
            Self::Stop(event) => event.to_collection_message("stop"),
        }
    }
}

impl TerminalCommandEvent {
    fn new(
        target: TerminalTarget,
        id: Option<String>,
        status: impl Into<String>,
        command: Option<String>,
    ) -> Self {
        Self {
            target,
            id,
            status: status.into(),
            command,
            exit_code: None,
            output: Vec::new(),
            output_path: None,
            is_error: false,
        }
    }

    fn tool_name(&self) -> &'static str {
        match self.target {
            TerminalTarget::Pty => "pty_start",
            TerminalTarget::BackgroundTask => "bash",
        }
    }

    fn label(&self) -> &'static str {
        match self.target {
            TerminalTarget::Pty => "pty",
            TerminalTarget::BackgroundTask => "background task",
        }
    }

    fn to_result_message(&self) -> String {
        let id = self.id.as_deref().unwrap_or("<unknown>");
        let mut rendered = match self.command.as_deref() {
            Some(command) if !command.is_empty() => {
                format!("{} {id} {}: {command}", self.label(), self.status)
            }
            _ => format!("{} {id} {}", self.label(), self.status),
        };
        if let Some(exit_code) = self.exit_code {
            rendered.push_str(&format!("\nexit_code: {exit_code}"));
        }
        if let Some(output_path) = self.output_path.as_deref() {
            rendered.push_str(&format!("\noutput: {output_path}"));
        } else if !self.output.is_empty() {
            rendered.push_str("\noutput:\n");
            rendered.push_str(&self.output.join("\n"));
        }
        rendered
    }
}

impl TerminalOutputDeltaEvent {
    fn tool_name(&self) -> &'static str {
        match self.target {
            TerminalTarget::Pty => "pty",
            TerminalTarget::BackgroundTask => "background task",
        }
    }
}

impl TerminalCollectionEvent {
    fn to_collection_message(&self, action: &str) -> String {
        let label = match self.target {
            TerminalTarget::Pty => "pty",
            TerminalTarget::BackgroundTask => "background task",
        };
        let collection_label = match (self.target, action) {
            (TerminalTarget::Pty, "list") => "pty sessions",
            (TerminalTarget::BackgroundTask, "list") => "background tasks",
            _ => label,
        };
        if self.items.is_empty() {
            return match action {
                "stop" => format!("{label} stopped: none"),
                _ => format!("{collection_label}: none"),
            };
        }

        let mut lines = match action {
            "stop" => vec![format!("{label} stopped: {}", self.items.len())],
            _ => vec![format!("{collection_label}: {}", self.items.len())],
        };
        for item in self.items.iter().take(6) {
            let id = item.id.as_deref().unwrap_or("<unknown>");
            let command = item.command.as_deref().unwrap_or("command unavailable");
            lines.push(format!("  {label} {id} {}: {command}", item.status));
        }
        let remaining = self.items.len().saturating_sub(6);
        if remaining > 0 {
            lines.push(format!("... {remaining} more"));
        }
        lines.join("\n")
    }
}

fn background_bash_start_event(
    value: &serde_json::Value,
    is_error: bool,
) -> Option<TerminalCommandEvent> {
    let task_id = value
        .get("background_task_id")
        .and_then(serde_json::Value::as_str)?;
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let output_path = value
        .get("output_path")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    let mut event = TerminalCommandEvent::new(
        TerminalTarget::BackgroundTask,
        Some(task_id.to_string()),
        status,
        None,
    );
    event.output_path = output_path;
    event.is_error = is_error;
    Some(event)
}

fn pty_command_event(value: &serde_json::Value, is_error: bool) -> TerminalCommandEvent {
    let mut event = TerminalCommandEvent::new(
        TerminalTarget::Pty,
        value
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown"),
        value
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(compact_instruction),
    );
    event.output = value
        .get("output")
        .and_then(serde_json::Value::as_str)
        .and_then(output_tail_preview)
        .unwrap_or_default();
    event.is_error = is_error;
    event
}

fn background_task_event(value: &serde_json::Value, is_error: bool) -> TerminalCommandEvent {
    let mut event = TerminalCommandEvent::new(
        TerminalTarget::BackgroundTask,
        value
            .get("task_id")
            .or_else(|| value.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown"),
        value
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(compact_instruction),
    );
    event.exit_code = value.get("exit_code").and_then(serde_json::Value::as_i64);
    event.output = value
        .get("output")
        .and_then(serde_json::Value::as_str)
        .and_then(output_tail_preview)
        .unwrap_or_default();
    event.is_error = is_error;
    event
}

fn collection_event(
    target: TerminalTarget,
    value: Option<&serde_json::Value>,
    is_error: bool,
) -> TerminalCollectionEvent {
    let items = value
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(|item| collection_item_event(target, item, is_error))
        .collect::<Vec<_>>();
    TerminalCollectionEvent { target, items }
}

fn collection_item_event(
    target: TerminalTarget,
    value: &serde_json::Value,
    is_error: bool,
) -> TerminalCommandEvent {
    let mut event = TerminalCommandEvent::new(
        target,
        value
            .get("session_id")
            .or_else(|| value.get("task_id"))
            .or_else(|| value.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown"),
        value
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(compact_instruction),
    );
    event.is_error = is_error;
    event
}

pub(crate) fn output_tail_preview(output: &str) -> Option<Vec<String>> {
    const TAIL_LIMIT: usize = 6;

    let mut lines = VecDeque::with_capacity(TAIL_LIMIT);
    for line in output.lines() {
        let sanitized = sanitize_terminal_output_line(line);
        if sanitized.trim().is_empty() {
            continue;
        }
        if lines.len() == TAIL_LIMIT {
            lines.pop_front();
        }
        lines.push_back(sanitized);
    }

    if lines.is_empty() {
        return None;
    }

    Some(lines.into_iter().collect())
}

pub(crate) fn sanitize_terminal_output_line(line: &str) -> String {
    crate::tui::display_sanitize::sanitize_display_line(line)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        TerminalEvent, TerminalTarget, output_tail_preview, sanitize_terminal_output_line,
    };

    #[test]
    fn builds_background_start_event_from_bash_result() {
        let event = TerminalEvent::from_tool_result(
            "bash",
            &json!({
                "background_task_id": "task-1",
                "status": "running",
                "output_path": "/tmp/rara.log"
            })
            .to_string(),
            false,
        )
        .expect("terminal event");

        assert_eq!(event.transcript_role(), "Tool Result");
        assert_eq!(
            event.to_transcript_message(),
            "background task task-1 running\noutput: /tmp/rara.log"
        );
    }

    #[test]
    fn sanitizes_pty_output_event_tail() {
        let event = TerminalEvent::from_tool_result(
            "pty_status",
            &json!({
                "session_id": "pty-1",
                "status": "completed",
                "command": "cargo test",
                "output": "\u{1b}[31mred\u{1b}[0m\r\nok\n"
            })
            .to_string(),
            false,
        )
        .expect("terminal event");

        assert_eq!(
            event.to_transcript_message(),
            "pty pty-1 completed: cargo test\noutput:\nred\nok"
        );
    }

    #[test]
    fn sanitizer_removes_ansi_and_non_printing_control_characters() {
        assert_eq!(
            sanitize_terminal_output_line(
                "\u{1b}[4munder\u{1b}[0m\u{8}line\u{7}\u{1b}]0;title\u{7}\tend\r"
            ),
            "underline    end"
        );
    }

    #[test]
    fn skips_progress_event_when_sanitized_chunk_is_empty() {
        let event = TerminalEvent::from_tool_progress(
            "bash",
            rara_tools::tool::ToolOutputStream::Stderr,
            "\u{1b}]0;title\u{7}\u{7}\r\n",
        );

        assert!(event.is_none());
    }

    #[test]
    fn builds_pty_write_result_as_terminal_event() {
        let event = TerminalEvent::from_tool_result(
            "pty_write",
            &json!({
                "session_id": "pty-1",
                "status": "running",
                "command": "cargo test",
                "output": "line 1\nline 2\n"
            })
            .to_string(),
            false,
        )
        .expect("terminal event");

        match event {
            TerminalEvent::End(command) => {
                assert_eq!(command.target, TerminalTarget::Pty);
                assert_eq!(command.id.as_deref(), Some("pty-1"));
                assert_eq!(command.status, "running");
                assert_eq!(
                    command.output,
                    vec!["line 1".to_string(), "line 2".to_string()]
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn output_tail_preview_keeps_only_last_non_empty_lines() {
        let output = (1..=10)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            output_tail_preview(&output),
            Some(vec![
                "line 5".to_string(),
                "line 6".to_string(),
                "line 7".to_string(),
                "line 8".to_string(),
                "line 9".to_string(),
                "line 10".to_string(),
            ])
        );
    }

    #[test]
    fn builds_background_use_event_only_for_background_bash() {
        let event = TerminalEvent::from_tool_use(
            "bash",
            &json!({
                "command": "sleep 10",
                "run_in_background": true
            }),
        )
        .expect("terminal event");

        match event {
            TerminalEvent::Begin(command) => {
                assert_eq!(command.target, TerminalTarget::BackgroundTask);
                assert_eq!(command.command.as_deref(), Some("sleep 10"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
