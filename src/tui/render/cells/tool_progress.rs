use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::tui::render::display_width;
use crate::tui::theme::{STATUS_WARNING, TEXT_SECONDARY, TOOL_STDERR_FG};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolProgressStream {
    Stdout,
    Stderr,
}

struct ToolProgressMarker {
    stream: ToolProgressStream,
    label: Option<String>,
}

pub(super) fn tool_progress_lines(
    message: &str,
    max_lines: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let (stdout_lines, stderr_lines) = split_tool_progress_streams(message);
    if stderr_lines.is_empty() {
        return stdout_progress_lines(&stdout_lines, max_lines);
    }

    let stderr_budget = tool_progress_stderr_budget(!stdout_lines.is_empty(), max_lines);
    let stderr_rendered = stderr_progress_lines(&stderr_lines, stderr_budget, width);
    let stdout_budget = max_lines.saturating_sub(stderr_rendered.len());
    let mut lines = if stdout_lines.is_empty() || stdout_budget == 0 {
        Vec::new()
    } else {
        stdout_progress_lines(&stdout_lines, stdout_budget)
    };

    lines.extend(stderr_rendered);
    lines
}

fn stdout_progress_lines(stdout_lines: &[String], max_lines: usize) -> Vec<Line<'static>> {
    if max_lines == 0 {
        return Vec::new();
    }
    if stdout_lines.is_empty() {
        return vec![Line::from(Span::styled(
            "…",
            Style::default()
                .fg(STATUS_WARNING)
                .add_modifier(Modifier::BOLD),
        ))];
    }

    if max_lines == usize::MAX || stdout_lines.len() <= max_lines {
        let mut lines = Vec::new();
        let mut visible = stdout_lines.iter().map(String::as_str);
        if let Some(first) = visible.next() {
            lines.push(Line::from(vec![
                Span::styled(
                    "…",
                    Style::default()
                        .fg(STATUS_WARNING)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {first}")),
            ]));
        }
        lines.extend(visible.map(|line| Line::from(format!("  {line}"))));
        return lines;
    }

    if max_lines == 1 {
        return vec![Line::from(vec![
            Span::styled(
                "…",
                Style::default()
                    .fg(STATUS_WARNING)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ... {} more line(s)", stdout_lines.len()),
                Style::default().fg(TEXT_SECONDARY),
            ),
        ])];
    }

    let content_budget = max_lines - 1;
    let hidden_count = stdout_lines.len().saturating_sub(content_budget);
    let mut visible = Vec::new();
    if content_budget == 1 {
        visible.push(stdout_lines[0].as_str());
    } else {
        visible.push(stdout_lines[0].as_str());
        visible.extend(
            stdout_lines[stdout_lines
                .len()
                .saturating_sub(content_budget.saturating_sub(1))..]
                .iter()
                .map(String::as_str),
        );
    }

    let mut lines = Vec::new();
    if let Some(first) = visible.first() {
        lines.push(Line::from(vec![
            Span::styled(
                "…",
                Style::default()
                    .fg(STATUS_WARNING)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {first}")),
        ]));
    }
    if hidden_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ... {hidden_count} more line(s)"),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
    lines.extend(
        visible
            .iter()
            .skip(1)
            .map(|line| Line::from(format!("  {line}"))),
    );
    lines
}

fn split_tool_progress_streams(message: &str) -> (Vec<String>, Vec<String>) {
    let mut current_stream = ToolProgressStream::Stdout;
    let mut stdout_lines = Vec::new();
    let mut stderr_lines = Vec::new();

    for line in message.lines() {
        if let Some(marker) = tool_progress_stream_marker(line) {
            current_stream = marker.stream;
            if let Some(label) = marker.label {
                match marker.stream {
                    ToolProgressStream::Stdout => stdout_lines.push(label),
                    ToolProgressStream::Stderr => stderr_lines.push(label),
                }
            }
            continue;
        }

        match current_stream {
            ToolProgressStream::Stdout => stdout_lines.push(line.to_string()),
            ToolProgressStream::Stderr => stderr_lines.push(line.to_string()),
        }
    }

    (stdout_lines, stderr_lines)
}

fn tool_progress_stream_marker(line: &str) -> Option<ToolProgressMarker> {
    let trimmed = line.trim();
    if trimmed == "background task stdout:" {
        return Some(ToolProgressMarker {
            stream: ToolProgressStream::Stdout,
            label: None,
        });
    }
    if trimmed == "background task stderr:" {
        return Some(ToolProgressMarker {
            stream: ToolProgressStream::Stderr,
            label: None,
        });
    }
    if trimmed.ends_with(" stdout:") {
        return Some(ToolProgressMarker {
            stream: ToolProgressStream::Stdout,
            label: Some(line.to_string()),
        });
    }
    if trimmed.ends_with(" stderr:") {
        return Some(ToolProgressMarker {
            stream: ToolProgressStream::Stderr,
            label: Some(line.to_string()),
        });
    }
    None
}

fn tool_progress_stderr_budget(has_stdout: bool, max_lines: usize) -> usize {
    if max_lines == 0 {
        return 0;
    }
    if !has_stdout || max_lines == 1 {
        return max_lines;
    }
    max_lines - 1
}

fn stderr_progress_lines(
    stderr_lines: &[String],
    max_lines: usize,
    width: u16,
) -> Vec<Line<'static>> {
    if max_lines == 0 {
        return Vec::new();
    }

    if max_lines == usize::MAX || stderr_lines.len() <= max_lines {
        return stderr_lines
            .iter()
            .map(|line| styled_stderr_progress_line(line, width))
            .collect();
    }

    if max_lines == 1 {
        return vec![styled_stderr_progress_line(
            &format!("... {} more stderr line(s)", stderr_lines.len()),
            width,
        )];
    }

    let visible_count = max_lines - 1;
    let hidden_count = stderr_lines.len().saturating_sub(visible_count);
    let mut rendered = vec![styled_stderr_progress_line(
        &format!("... {hidden_count} more stderr line(s)"),
        width,
    )];
    rendered.extend(
        stderr_lines[stderr_lines.len().saturating_sub(visible_count)..]
            .iter()
            .map(|line| styled_stderr_progress_line(line, width)),
    );
    rendered
}

fn styled_stderr_progress_line(text: &str, width: u16) -> Line<'static> {
    let text = format!("  {text}");
    let target_width = usize::from(width.saturating_sub(2)).max(display_width(&text));
    let padding = target_width.saturating_sub(display_width(&text));
    Line::from(Span::styled(
        format!("{text}{}", " ".repeat(padding)),
        Style::default().fg(TOOL_STDERR_FG),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn stdout_progress_respects_line_budget_when_truncated() {
        let rendered = tool_progress_lines(
            [
                "background task stdout:",
                "line 1",
                "line 2",
                "line 3",
                "line 4",
                "line 5",
            ]
            .join("\n")
            .as_str(),
            3,
            80,
        );
        let rendered_text = plain(&rendered);

        assert_eq!(rendered.len(), 3);
        assert!(!rendered_text.contains("background task stdout:"));
        assert!(rendered_text.contains("line 1"));
        assert!(rendered_text.contains("... 3 more line(s)"));
        assert!(rendered_text.contains("line 5"));
    }

    #[test]
    fn stderr_progress_uses_full_budget_without_stdout_and_marks_truncation() {
        let rendered = tool_progress_lines(
            [
                "background task stderr:",
                "err 1",
                "err 2",
                "err 3",
                "err 4",
            ]
            .join("\n")
            .as_str(),
            3,
            80,
        );
        let rendered_text = plain(&rendered);

        assert_eq!(rendered.len(), 3);
        assert!(rendered_text.contains("... 2 more stderr line(s)"));
        assert!(rendered_text.contains("err 3"));
        assert!(rendered_text.contains("err 4"));
        assert!(rendered.iter().all(|line| {
            line.spans
                .iter()
                .any(|span| span.style.fg == Some(TOOL_STDERR_FG))
        }));
    }

    #[test]
    fn custom_stderr_label_is_preserved_in_stderr_card() {
        let rendered = tool_progress_lines(
            [
                "compiler stderr:",
                "warning: unused import",
                "background task stdout:",
                "done",
            ]
            .join("\n")
            .as_str(),
            4,
            80,
        );
        let rendered_text = plain(&rendered);

        assert!(!rendered_text.contains("background task stdout:"));
        assert!(rendered_text.contains("compiler stderr:"));
        assert!(rendered_text.contains("warning: unused import"));
        assert!(
            rendered_text.find("done").unwrap() < rendered_text.find("compiler stderr:").unwrap()
        );
    }
}
