// Auto-split from cells_components.rs
use std::path::Path;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::tool_progress::tool_progress_lines;
use super::{HistoryCell, InteractionCompletionKind};
use crate::tui::interaction_text::{
    pending_interaction_card_title, status_planning_suggestion_text,
};
use crate::tui::markdown_render::render_markdown_text_with_width;
use crate::tui::plan_display::updated_plan_lines;
use crate::tui::queued_input::{
    QueuedFollowUpSection, pending_follow_up_heading, queued_follow_up_heading,
};
use crate::tui::render::diff::render_patch_preview;
use crate::tui::render::{
    display_width, formatted_message_lines, prefixed_message_lines, rendered_markdown_lines,
    section_label, startup_card_inner_width, truncate_for_startup_card, truncate_path_middle,
    with_border,
};
use crate::tui::state::{ActivePendingInteractionKind, TuiApp};
use crate::tui::sub_agent_display::SUB_AGENT_QUESTION_COLOR;
use crate::tui::theme::*;

pub(crate) struct RespondingCell<'a> {
    content: RespondingCellContent<'a>,
}

enum RespondingCellContent<'a> {
    Stream {
        lines: &'a [Line<'static>],
        max_lines: usize,
    },
    CompactMessage {
        message: String,
        max_lines: usize,
    },
    Message {
        role: &'static str,
        message: &'a str,
        max_lines: usize,
        cwd: Option<&'a Path>,
    },
    ToolResult {
        role: &'a str,
        message: &'a str,
        max_lines: usize,
    },
    Working(&'a str),
}

impl<'a> RespondingCell<'a> {
    pub(crate) fn from_stream(stream_lines: &'a [Line<'static>]) -> Self {
        Self {
            content: RespondingCellContent::Stream {
                lines: stream_lines,
                max_lines: usize::MAX,
            },
        }
    }

    pub(crate) fn from_stream_compact(stream_lines: &'a [Line<'static>], max_lines: usize) -> Self {
        Self {
            content: RespondingCellContent::Stream {
                lines: stream_lines,
                max_lines,
            },
        }
    }

    pub(crate) fn from_message(
        role: &'static str,
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
        }
    }

    pub(crate) fn from_compact_message(message: String, max_lines: usize) -> Self {
        Self {
            content: RespondingCellContent::CompactMessage { message, max_lines },
        }
    }

    pub(crate) fn from_tool_result(role: &'a str, message: &'a str, max_lines: usize) -> Self {
        Self {
            content: RespondingCellContent::ToolResult {
                role,
                message,
                max_lines,
            },
        }
    }

    pub(crate) fn working(detail: &'a str) -> Self {
        Self {
            content: RespondingCellContent::Working(detail),
        }
    }
}

impl HistoryCell for RespondingCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        match &self.content {
            RespondingCellContent::Stream { lines, max_lines } => {
                lightweight_stream_lines(lines, *max_lines)
            }
            RespondingCellContent::Message {
                role,
                message,
                max_lines,
                cwd,
            } if *role == "Responding" => compact_message_lines(message, *max_lines),
            RespondingCellContent::CompactMessage { message, max_lines } => {
                compact_message_lines(message, *max_lines)
            }
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
            } if *role == "Tool Progress" => tool_progress_lines(message, *max_lines, width),
            RespondingCellContent::ToolResult {
                role,
                message,
                max_lines,
            } => prefixed_message_lines(role, message, *max_lines),
            RespondingCellContent::Working(detail) => compact_message_lines(detail, 1),
        }
    }
}

fn lightweight_stream_lines(rendered: &[Line<'static>], max_lines: usize) -> Vec<Line<'static>> {
    let mut lines = markdown_body_lines(rendered, max_lines);
    if lines.is_empty() {
        return vec![Line::from("•")];
    }

    if let Some(first) = lines.first_mut() {
        first.spans.insert(0, Span::raw("• "));
    }

    for line in lines.iter_mut().skip(1) {
        line.spans.insert(0, Span::raw("  "));
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

pub(crate) fn markdown_body_lines(
    rendered: &[Line<'static>],
    max_lines: usize,
) -> Vec<Line<'static>> {
    let mut lines = rendered_markdown_lines("Responding", rendered, max_lines);
    if !lines.is_empty() {
        lines.remove(0);
    }
    if lines.is_empty() {
        lines.push(Line::from(String::new()));
    }
    lines
}

fn responding_card_lines(
    title: &'static str,
    mut body_lines: Vec<Line<'static>>,
    width: u16,
) -> Vec<Line<'static>> {
    if body_lines.is_empty() {
        body_lines.push(Line::from(String::new()));
    }

    let available_inner_width = usize::from(width.saturating_sub(4).max(1));
    let inner_width = body_lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|span| display_width(span.content.as_ref()))
                .sum::<usize>()
        })
        .max()
        .unwrap_or(1)
        .clamp(1, available_inner_width.max(1));

    let mut lines = vec![Line::from(section_label(title, PHASE_PLANNING))];
    lines.extend(with_border(body_lines, inner_width));
    lines
}
