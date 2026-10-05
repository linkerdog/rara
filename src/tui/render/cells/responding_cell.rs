// Auto-split from cells_components.rs
use std::path::Path;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::tool_progress::tool_progress_lines;
use super::{HistoryCell, InteractionCompletionKind, LspDiagnosticsCell};
use crate::tui::interaction_text::{
    pending_interaction_card_title, status_planning_suggestion_text,
};
use crate::tui::markdown_render::render_markdown_text_with_width_and_cwd;
use crate::tui::message_role::MessageRole;
use crate::tui::plan_display::updated_plan_lines;
use crate::tui::queued_input::{
    QueuedFollowUpSection, pending_follow_up_heading, queued_follow_up_heading,
};
use crate::tui::render::diff::render_message_diff_preview;
use crate::tui::render::{
    formatted_message_lines, prefixed_message_lines, startup_card_inner_width,
    truncate_for_startup_card, truncate_path_middle,
};
use crate::tui::state::{ActivePendingInteractionKind, TuiApp};
use crate::tui::sub_agent_display::SUB_AGENT_QUESTION_COLOR;
use crate::tui::theme::*;

pub(crate) struct RespondingCell<'a> {
    content: RespondingCellContent<'a>,
    #[cfg(test)]
    work: Option<crate::tui::transcript_work::WorkMeter>,
}

enum RespondingCellContent<'a> {
    #[cfg(test)]
    Stream {
        lines: &'a [Line<'static>],
        max_lines: usize,
    },
    CompactMessage {
        message: String,
        max_lines: usize,
        cwd: Option<&'a Path>,
    },
    Message {
        role: MessageRole,
        message: &'a str,
        max_lines: usize,
        cwd: Option<&'a Path>,
    },
    ToolResult {
        role: &'a MessageRole,
        message: &'a str,
        max_lines: usize,
    },
    Working(&'a str),
}

impl<'a> RespondingCell<'a> {
    pub(crate) fn stream_body_line(line: &Line<'static>, index: usize) -> Line<'static> {
        let mut spans = vec![
            Span::raw(if index == 0 { "• " } else { "  " }),
            Span::raw("  "),
        ];
        spans.extend(line.spans.clone());
        Line::from(spans).style(line.style)
    }

    pub(crate) fn stream_summary_line(remaining: usize) -> Line<'static> {
        let mut line = super::super::markdown_truncation_line(remaining);
        line.spans.insert(0, Span::raw("  "));
        line
    }
    #[cfg(test)]
    pub(crate) fn from_stream(stream_lines: &'a [Line<'static>]) -> Self {
        Self {
            content: RespondingCellContent::Stream {
                lines: stream_lines,
                max_lines: usize::MAX,
            },
            #[cfg(test)]
            work: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_stream_compact(stream_lines: &'a [Line<'static>], max_lines: usize) -> Self {
        Self {
            content: RespondingCellContent::Stream {
                lines: stream_lines,
                max_lines,
            },
            #[cfg(test)]
            work: None,
        }
    }

    pub(crate) fn from_message(
        role: MessageRole,
        message: &'a str,
        max_lines: usize,
        cwd: Option<&'a Path>,
    ) -> Self {
        Self {
            content: RespondingCellContent::Message {
                role,
                message,
                max_lines,
                cwd,
            },
            #[cfg(test)]
            work: None,
        }
    }

    pub(crate) fn from_compact_message(
        message: String,
        max_lines: usize,
        cwd: Option<&'a Path>,
    ) -> Self {
        Self {
            content: RespondingCellContent::CompactMessage {
                message,
                max_lines,
                cwd,
            },
            #[cfg(test)]
            work: None,
        }
    }

    pub(crate) fn from_tool_result(
        role: &'a MessageRole,
        message: &'a str,
        max_lines: usize,
    ) -> Self {
        Self {
            content: RespondingCellContent::ToolResult {
                role,
                message,
                max_lines,
            },
            #[cfg(test)]
            work: None,
        }
    }

    pub(crate) fn working(detail: &'a str) -> Self {
        Self {
            content: RespondingCellContent::Working(detail),
            #[cfg(test)]
            work: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_work_meter(mut self, work: crate::tui::transcript_work::WorkMeter) -> Self {
        self.work = Some(work);
        self
    }
}

impl HistoryCell for RespondingCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        match &self.content {
            #[cfg(test)]
            RespondingCellContent::Stream { lines, max_lines } => {
                #[cfg(test)]
                if let Some(work) = &self.work {
                    work.record(
                        crate::tui::transcript_work::WorkKind::Clone,
                        lines.len().min(*max_lines),
                    );
                }
                lightweight_stream_lines(lines, *max_lines)
            }
            RespondingCellContent::Message {
                role,
                message,
                max_lines,
                cwd,
            } if *role == MessageRole::Responding => compact_message_lines(message, *max_lines),
            RespondingCellContent::CompactMessage {
                message,
                max_lines,
                cwd,
            } => compact_markdown_message_lines(message, *max_lines, *cwd),
            RespondingCellContent::Message {
                role,
                message,
                max_lines,
                cwd,
            } => formatted_message_lines(role, message, *max_lines, *cwd),
            RespondingCellContent::ToolResult {
                role,
                message,
                max_lines,
            } if **role == MessageRole::ToolProgress => {
                tool_progress_lines(message, *max_lines, width)
            }
            RespondingCellContent::ToolResult {
                role,
                message,
                max_lines,
            } => {
                if let Some(lines) = bash_completion_lines(role, message, *max_lines) {
                    lines
                } else if let Some(cell) = LspDiagnosticsCell::from_message(message) {
                    cell.display_lines(width)
                } else if let Some(lines) =
                    render_message_diff_preview(Some(role.as_str()), message, width)
                {
                    lines
                } else {
                    prefixed_message_lines(role, message, *max_lines)
                }
            }
            RespondingCellContent::Working(detail) => compact_message_lines(detail, 1),
        }
    }
}

fn bash_completion_lines(
    role: &MessageRole,
    message: &str,
    max_lines: usize,
) -> Option<Vec<Line<'static>>> {
    if !matches!(role, MessageRole::ToolResult | MessageRole::ToolError) {
        return None;
    }

    let mut lines = message.lines();
    let first = lines.next()?.trim();
    let normalized = first.strip_prefix("bash: ").unwrap_or(first);
    let (success, exit_code) = if normalized == "bash finished with exit code 0"
        || normalized == "finished with exit code 0"
    {
        (true, Some(0))
    } else {
        let code = normalized
            .strip_prefix("bash failed with exit code ")
            .or_else(|| normalized.strip_prefix("failed with exit code "))
            .and_then(|value| value.parse::<i32>().ok())?;
        (false, Some(code))
    };

    let icon_style = if success {
        Style::default()
            .fg(STATUS_SUCCESS)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(STATUS_ERROR)
            .add_modifier(Modifier::BOLD)
    };
    let mut status = Line::from(vec![
        Span::styled(if success { "✓" } else { "✗" }, icon_style),
        Span::raw(" bash"),
    ]);
    if let Some(code) = exit_code.filter(|code| *code != 0) {
        status.push_span(Span::styled(
            format!(" · exit {code}"),
            Style::default().fg(TEXT_SECONDARY),
        ));
    }

    let body_budget = max_lines.saturating_sub(1);
    let mut rendered = vec![status];
    if body_budget == 0 {
        return Some(rendered);
    }

    let body_lines = lines
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let truncated = body_lines.len() > body_budget;
    let capped = if truncated {
        body_budget.saturating_sub(1)
    } else {
        body_lines.len().min(body_budget)
    };
    rendered.extend(body_lines.iter().take(capped).map(|line| {
        Line::from(Span::styled(
            format!("  {line}"),
            Style::default().fg(TEXT_SECONDARY),
        ))
    }));
    if truncated {
        rendered.push(Line::from(Span::styled(
            format!("  ... {} more line(s)", body_lines.len() - capped),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }
    Some(rendered)
}

#[cfg(test)]
fn lightweight_stream_lines(rendered: &[Line<'static>], max_lines: usize) -> Vec<Line<'static>> {
    if rendered.is_empty() {
        return vec![Line::from("• ")];
    }
    let cap = rendered.len().min(max_lines);
    let mut lines = rendered
        .iter()
        .take(cap)
        .enumerate()
        .map(|(index, line)| RespondingCell::stream_body_line(line, index))
        .collect::<Vec<_>>();
    if cap < rendered.len() {
        let mut summary = RespondingCell::stream_summary_line(rendered.len() - cap);
        if lines.is_empty()
            && let Some(first) = summary.spans.first_mut()
        {
            *first = Span::raw("• ");
        }
        lines.push(summary);
    }
    lines
}

fn compact_message_lines(message: &str, max_lines: usize) -> Vec<Line<'static>> {
    let message_lines = message.lines().collect::<Vec<_>>();
    if message_lines.is_empty() {
        return vec![Line::from("•")];
    }

    let capped = if max_lines == usize::MAX {
        message_lines.len()
    } else {
        max_lines.min(message_lines.len())
    };

    let mut lines = message_lines
        .iter()
        .take(capped)
        .map(|line| Line::from(format!("• {line}")))
        .collect::<Vec<_>>();

    if message_lines.len() > capped {
        lines.push(Line::from(Span::styled(
            format!("  ... {} more line(s)", message_lines.len() - capped),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }

    lines
}

fn compact_markdown_message_lines(
    message: &str,
    max_lines: usize,
    cwd: Option<&Path>,
) -> Vec<Line<'static>> {
    let message_lines = message.lines().collect::<Vec<_>>();
    if message_lines.is_empty() {
        return vec![Line::from("•")];
    }

    let capped = if max_lines == usize::MAX {
        message_lines.len()
    } else {
        max_lines.min(message_lines.len())
    };

    let mut lines = Vec::new();
    for line in message_lines.iter().take(capped) {
        let rendered = render_markdown_text_with_width_and_cwd(line, None, cwd);
        let mut body = rendered.lines;
        if body.is_empty() {
            lines.push(Line::from("•"));
            continue;
        }

        if let Some(first) = body.first_mut() {
            first.spans.insert(0, Span::raw("• "));
        }
        for continuation in body.iter_mut().skip(1) {
            continuation.spans.insert(0, Span::raw("  "));
        }
        lines.extend(body);
    }

    if message_lines.len() > capped {
        lines.push(Line::from(Span::styled(
            format!("  ... {} more line(s)", message_lines.len() - capped),
            Style::default().fg(TEXT_SECONDARY),
        )));
    }

    lines
}
