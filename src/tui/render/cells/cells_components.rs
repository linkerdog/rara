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
    section_span, startup_card_inner_width, truncate_for_startup_card, truncate_path_middle,
    with_border,
};
use crate::tui::state::{ActivePendingInteractionKind, TuiApp};
use crate::tui::sub_agent_display::SUB_AGENT_QUESTION_COLOR;
use crate::tui::theme::*;

pub(super) struct UserCell {
    message: String,
}

impl UserCell {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl HistoryCell for UserCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        prefixed_message_lines("You", &self.message, 4)
    }
}

struct SummaryCell {
    title: &'static str,
    color: Color,
    summary: String,
}

impl SummaryCell {
    fn new(title: &'static str, color: Color, summary: impl Into<String>) -> Self {
        Self {
            title,
            color,
            summary: summary.into(),
        }
    }
}

impl HistoryCell for SummaryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(section_span(self.title, self.color))];
        let mut summary_lines = self.summary.lines();
        while let Some(line) = summary_lines.next() {
            if line.trim_start() == "diff:" {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "diff:",
                        Style::default()
                            .fg(PHASE_PLANNING)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                let diff = summary_lines
                    .map(|line| line.trim_start())
                    .collect::<Vec<_>>()
                    .join("\n");
                lines.extend(render_patch_preview(diff.as_str(), width));
                break;
            }
            lines.push(Line::from(format!("  {line}")));
        }
        lines
    }
}
macro_rules! summary_cell {
    ($name:ident, $title:expr, $color:expr) => {
        pub(super) struct $name {
            inner: SummaryCell,
        }
        impl $name {
            pub(super) fn new(summary: impl Into<String>) -> Self {
                Self {
                    inner: SummaryCell::new($title, $color, summary),
                }
            }
        }
        impl HistoryCell for $name {
            fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
                self.inner.display_lines(width)
            }
        }
    };
    ($name:ident, $active_title:expr, $active_color:expr, $done_title:expr, $done_color:expr) => {
        pub(super) struct $name {
            inner: SummaryCell,
        }
        impl $name {
            pub(super) fn new(summary: impl Into<String>, active: bool) -> Self {
                let (title, color) = if active {
                    ($active_title, $active_color)
                } else {
                    ($done_title, $done_color)
                };
                Self {
                    inner: SummaryCell::new(title, color, summary),
                }
            }
        }
        impl HistoryCell for $name {
            fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
                self.inner.display_lines(width)
            }
        }
    };
}

summary_cell!(ExploredCell, "Explored", PHASE_EXPLORED);
summary_cell!(RanCell, "Ran", PHASE_RAN);

pub(super) struct PlanSummaryCell {
    steps: Vec<(String, String)>,
    explanation: Option<String>,
}

impl PlanSummaryCell {
    pub(super) fn new(steps: Vec<(String, String)>, explanation: Option<String>) -> Self {
        Self { steps, explanation }
    }
}

impl HistoryCell for PlanSummaryCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        updated_plan_lines(self.steps.as_slice(), self.explanation.as_deref())
    }
}

summary_cell!(
    PlanningCell,
    "Planning",
    PHASE_PLANNING,
    "Planned",
    PHASE_PLANNING
);
summary_cell!(
    ExploringCell,
    "Exploring",
    PHASE_EXPLORING,
    "Explored",
    PHASE_EXPLORED
);
summary_cell!(RunningCell, "Running", PHASE_RUNNING, "Ran", PHASE_RAN);

pub(super) struct ThinkingTextCell {
    message: String,
    max_lines: usize,
}

impl ThinkingTextCell {
    pub(super) fn new(message: &str, max_lines: usize) -> Self {
        Self {
            message: message.to_string(),
            max_lines,
        }
    }
}

impl HistoryCell for ThinkingTextCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let render_width = usize::from(width.saturating_sub(2));
        let rendered = render_markdown_text_with_width(&self.message, Some(render_width));
        let rendered_lines = rendered.lines;
        let start = rendered_lines.len().saturating_sub(self.max_lines);
        let body = markdown_body_lines(&rendered_lines[start..], self.max_lines);
        let mut lines = vec![Line::from(section_span("Thinking", PHASE_THINKING))];
        if start > 0 {
            lines.push(Line::from(Span::styled(
                format!("  ... {start} more line(s)"),
                Style::default().fg(TEXT_SECONDARY),
            )));
        }
        lines.extend(body.into_iter().map(|mut line| {
            line.spans.insert(0, Span::raw("  "));
            line
        }));
        lines
    }
}

pub(super) struct ThinkingGroupCell<'a> {
    message: String,
    stream_lines: Option<&'a [Line<'static>]>,
    max_lines: usize,
}

impl<'a> ThinkingGroupCell<'a> {
    pub(super) fn new(
        message: String,
        stream_lines: Option<&'a [Line<'static>]>,
        max_lines: usize,
    ) -> Self {
        Self {
            message,
            stream_lines,
            max_lines,
        }
    }
}

impl HistoryCell for ThinkingGroupCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let render_width = usize::from(width.saturating_sub(2));
        let mut rendered_lines = Vec::new();
        if !self.message.is_empty() {
            let rendered = render_markdown_text_with_width(&self.message, Some(render_width));
            rendered_lines.extend(rendered.lines);
        }
        if let Some(stream_lines) = self.stream_lines {
            rendered_lines.extend(stream_lines.iter().cloned());
        }

        let start = rendered_lines.len().saturating_sub(self.max_lines);
        let body = markdown_body_lines(&rendered_lines[start..], self.max_lines);
        let mut lines = vec![Line::from(section_span("Thinking", PHASE_THINKING))];
        if start > 0 {
            lines.push(Line::from(Span::styled(
                format!("  ... {start} more line(s)"),
                Style::default().fg(TEXT_SECONDARY),
            )));
        }
        lines.extend(body.into_iter().map(|mut line| {
            line.spans.insert(0, Span::raw("  "));
            line
        }));
        lines
    }
}

pub(super) struct TerminalCell {
    command: String,
    output: Vec<String>,
    active: bool,
    success: Option<bool>,
}

impl TerminalCell {
    pub(super) fn new(
        command: impl Into<String>,
        output: Vec<String>,
        active: bool,
        success: Option<bool>,
    ) -> Self {
        Self {
            command: command.into(),
            output,
            active,
            success,
        }
    }

    fn status_icon(&self) -> &'static str {
        match (self.active, self.success) {
            (true, _) => "▶",
            (false, Some(true)) => "✓",
            (false, Some(false)) => "✕",
            (false, None) => "•",
        }
    }

    fn status_style(&self) -> Style {
        let color = match (self.active, self.success) {
            (true, _) => PHASE_EXPLORING,
            (false, Some(true)) => STATUS_SUCCESS,
            (false, Some(false)) => STATUS_ERROR,
            (false, None) => PHASE_RAN,
        };
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    }

    fn title(&self) -> &'static str {
        if self.active { "Running" } else { "Ran" }
    }

    fn stdout_stderr_split(&self) -> (Vec<String>, Vec<String>) {
        let stderr_prefix = "[stderr] ";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for line in &self.output {
            if let Some(rest) = line.strip_prefix(stderr_prefix) {
                stderr.push(rest.trim_end().to_string());
            } else {
                stdout.push(line.clone());
            }
        }
        (stdout, stderr)
    }

    fn fold_output(total: usize, visible: &[String], indent: &str) -> Vec<Line<'static>> {
        let edge = 3;
        let indent_owned = indent.to_string();
        if visible.len() <= edge * 2 + 1 {
            return visible
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    let prefix = if i == 0 { "  └ " } else { "    " };
                    Line::from(vec![
                        Span::styled(prefix, Style::default().fg(TEXT_SECONDARY)),
                        Span::styled(line.clone(), Style::default().fg(TEXT_SECONDARY)),
                    ])
                })
                .collect();
        }
        let omitted = visible.len() - edge * 2;
        let mut lines: Vec<Line<'static>> = visible
            .iter()
            .take(edge)
            .enumerate()
            .map(|(i, line)| {
                let prefix = if i == 0 { "  └ " } else { "    " };
                Line::from(vec![
                    Span::styled(prefix, Style::default().fg(TEXT_SECONDARY)),
                    Span::styled(line.clone(), Style::default().fg(TEXT_SECONDARY)),
                ])
            })
            .collect();
        lines.push(Line::from(Span::styled(
            format!("{indent}  ... {omitted} more line(s)  (showing {edge} + {edge} of {total})"),
            Style::default().fg(TEXT_SECONDARY),
        )));
        lines.extend(visible.iter().skip(visible.len() - edge).map(|line| {
            Line::from(vec![
                Span::styled(indent_owned.clone(), Style::default().fg(TEXT_SECONDARY)),
                Span::styled(line.clone(), Style::default().fg(TEXT_SECONDARY)),
            ])
        }));
        lines
    }
}

impl HistoryCell for TerminalCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Header: status icon + title + command
        lines.push(Line::from(vec![
            Span::styled(self.status_icon().to_string(), self.status_style()),
            Span::raw(" "),
            Span::styled(self.title(), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::raw(self.command.clone()),
        ]));

        let (stdout, stderr) = self.stdout_stderr_split();

        // Stdout section
        if !stdout.is_empty() {
            lines.extend(Self::fold_output(stdout.len(), &stdout, "    "));
        }

        // Stderr section with colored background
        if !stderr.is_empty() {
            let stderr_count = stderr.len();
            let edge = 3;
            let hidden = stderr_count > edge * 2 + 1;

            if hidden {
                for (i, line) in stderr.iter().take(edge).enumerate() {
                    let prefix = if i == 0 { "  └ " } else { "    " };
                    lines.push(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(TOOL_STDERR_FG)),
                        Span::styled(
                            line.clone(),
                            Style::default().bg(TOOL_STDERR_BG).fg(TOOL_STDERR_FG),
                        ),
                    ]));
                }
                let omitted = stderr_count - edge * 2;
                lines.push(Line::from(Span::styled(
                    format!(
                        "      ... {omitted} more stderr line(s)  (showing {edge} + {edge} of {stderr_count})"
                    ),
                    Style::default()
                        .fg(TOOL_STDERR_FG)
                        .bg(TOOL_STDERR_BG),
                )));
                for line in stderr.iter().skip(stderr_count - edge) {
                    lines.push(Line::from(vec![
                        Span::styled("    ", Style::default().fg(TOOL_STDERR_FG)),
                        Span::styled(
                            line.clone(),
                            Style::default().bg(TOOL_STDERR_BG).fg(TOOL_STDERR_FG),
                        ),
                    ]));
                }
            } else {
                for (i, line) in stderr.iter().enumerate() {
                    let prefix = if i == 0 { "  └ " } else { "    " };
                    lines.push(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(TOOL_STDERR_FG)),
                        Span::styled(
                            line.clone(),
                            Style::default().bg(TOOL_STDERR_BG).fg(TOOL_STDERR_FG),
                        ),
                    ]));
                }
            }
        }

        // Footer: status line when completed
        if !self.active {
            let exit_status = match self.success {
                Some(true) => Span::styled(
                    "exit: 0",
                    Style::default()
                        .fg(STATUS_SUCCESS)
                        .add_modifier(Modifier::BOLD),
                ),
                Some(false) => Span::styled(
                    "exit: non-zero",
                    Style::default()
                        .fg(STATUS_ERROR)
                        .add_modifier(Modifier::BOLD),
                ),
                None => Span::styled("done", Style::default().fg(TEXT_SECONDARY)),
            };
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(TEXT_SECONDARY)),
                Span::styled("┄", Style::default().fg(TEXT_SECONDARY)),
                Span::raw(" "),
                exit_status,
            ]));
        }

        lines
    }
}

struct ApprovalCell {
    title: &'static str,
    color: Color,
    lines: Vec<String>,
}

impl ApprovalCell {
    fn new(title: &'static str, color: Color, lines: Vec<String>) -> Self {
        Self {
            title,
            color,
            lines,
        }
    }
}

impl HistoryCell for ApprovalCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(section_span(self.title, self.color))];
        lines.extend(
            self.lines
                .iter()
                .map(|line| Line::from(format!("  {line}"))),
        );
        lines
    }
}

pub(super) struct PendingInteractionCell {
    inner: ApprovalCell,
}

impl PendingInteractionCell {
    pub(super) fn new(kind: ActivePendingInteractionKind, lines: Vec<String>) -> Self {
        let color = match kind {
            ActivePendingInteractionKind::PlanApproval
            | ActivePendingInteractionKind::PlanningQuestion => PHASE_PLANNING,
            ActivePendingInteractionKind::ShellApproval
            | ActivePendingInteractionKind::ExplorationQuestion => PHASE_EXPLORING,
            ActivePendingInteractionKind::SubAgentQuestion => SUB_AGENT_QUESTION_COLOR,
            ActivePendingInteractionKind::RequestInput => STATUS_SUCCESS,
        };
        Self {
            inner: ApprovalCell::new(pending_interaction_card_title(kind), color, lines),
        }
    }
}

impl HistoryCell for PendingInteractionCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.inner.display_lines(width)
    }
}

pub(super) struct PlanningSuggestionCell {
    text: String,
}

impl PlanningSuggestionCell {
    pub(super) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl HistoryCell for PlanningSuggestionCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(section_span(
            "Planning Suggested",
            PHASE_PLANNING,
        ))];
        lines.extend(
            self.text
                .lines()
                .map(|line| Line::from(format!("  {line}"))),
        );
        lines
    }
}

pub(super) struct QueuedFollowUpCell {
    sections: Vec<QueuedFollowUpSection>,
}

impl QueuedFollowUpCell {
    pub(super) fn new(sections: Vec<QueuedFollowUpSection>) -> Self {
        Self { sections }
    }
}

impl HistoryCell for QueuedFollowUpCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        self.sections
            .iter()
            .map(|section| {
                let phase = if section.title == pending_follow_up_heading() {
                    "after tool"
                } else if section.title == queued_follow_up_heading() {
                    "after turn"
                } else {
                    "queued"
                };
                let remaining = if section.remaining > 0 {
                    format!(" (+{})", section.remaining)
                } else {
                    String::new()
                };
                Line::from(vec![
                    section_span("Queued", PHASE_PLANNING),
                    Span::raw(format!(" · {phase} · {}{remaining}", section.preview)),
                ])
            })
            .collect()
    }
}

struct CompletionCell {
    title: String,
    color: Color,
    summary: String,
}

impl CompletionCell {
    fn new(title: impl Into<String>, color: Color, summary: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            color,
            summary: summary.into(),
        }
    }
}

pub(super) struct CommittedInteractionCell {
    inner: CompletionCell,
}

impl CommittedInteractionCell {
    pub(super) fn new(kind: InteractionCompletionKind, summary: impl Into<String>) -> Self {
        Self {
            inner: CompletionCell::new(kind.title(), kind.color(), summary),
        }
    }
}

impl HistoryCell for CommittedInteractionCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.inner.display_lines(width)
    }
}

impl HistoryCell for CompletionCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![
            Line::from(Span::styled(
                format!(" {} ", self.title),
                Style::default()
                    .fg(BADGE_FG_LIGHT)
                    .bg(self.color)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("  {}", self.summary)),
        ]
    }
}

pub(super) struct PlanModeCell;

impl HistoryCell for PlanModeCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![Line::from(section_span("Plan Mode", PHASE_PLANNING))]
    }
}

pub(super) struct RespondingCell<'a> {
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
    pub(super) fn from_stream(stream_lines: &'a [Line<'static>]) -> Self {
        Self {
            content: RespondingCellContent::Stream {
                lines: stream_lines,
                max_lines: usize::MAX,
            },
        }
    }

    pub(super) fn from_stream_compact(stream_lines: &'a [Line<'static>], max_lines: usize) -> Self {
        Self {
            content: RespondingCellContent::Stream {
                lines: stream_lines,
                max_lines,
            },
        }
    }

    pub(super) fn from_message(
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

    pub(super) fn from_compact_message(message: String, max_lines: usize) -> Self {
        Self {
            content: RespondingCellContent::CompactMessage { message, max_lines },
        }
    }

    pub(super) fn from_tool_result(role: &'a str, message: &'a str, max_lines: usize) -> Self {
        Self {
            content: RespondingCellContent::ToolResult {
                role,
                message,
                max_lines,
            },
        }
    }

    pub(super) fn working(detail: &'a str) -> Self {
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

fn markdown_body_lines(rendered: &[Line<'static>], max_lines: usize) -> Vec<Line<'static>> {
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

    let mut lines = vec![Line::from(section_span(title, PHASE_PLANNING))];
    lines.extend(with_border(body_lines, inner_width));
    lines
}

pub(super) struct MessageCell<'a> {
    role: &'a str,
    message: &'a str,
    max_lines: usize,
    cwd: Option<&'a Path>,
}

impl<'a> MessageCell<'a> {
    pub(super) fn new(
        role: &'a str,
        message: &'a str,
        max_lines: usize,
        cwd: Option<&'a Path>,
    ) -> Self {
        Self {
            role,
            message,
            max_lines,
            cwd,
        }
    }
}

impl HistoryCell for MessageCell<'_> {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        formatted_message_lines(self.role, self.message, self.max_lines, self.cwd)
    }
}

pub(crate) struct StartupCardCell {
    model_label: String,
    directory: String,
}

impl StartupCardCell {
    pub(crate) fn new(model_label: String, directory: String) -> Self {
        Self {
            model_label,
            directory,
        }
    }
}

impl HistoryCell for StartupCardCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let Some(inner_width) = startup_card_inner_width(width) else {
            return Vec::new();
        };

        let model_label = "model:";
        let directory_label = "directory:";
        let label_width = directory_label.len();
        let model_prefix = format!("{model_label:<label_width$} ");
        let hint = "/model to change";
        let hint_width = display_width(hint);
        let model_prefix_width = display_width(&model_prefix);
        let model_available_width = inner_width
            .saturating_sub(model_prefix_width)
            .saturating_sub(1)
            .saturating_sub(hint_width);
        let model_value = truncate_for_startup_card(&self.model_label, model_available_width);
        let model_value_width = display_width(&model_value);
        let gap_width = inner_width
            .saturating_sub(model_prefix_width)
            .saturating_sub(model_value_width)
            .saturating_sub(hint_width)
            .max(1);
        let directory_prefix = format!("{directory_label:<label_width$} ");
        let directory_max_width = inner_width.saturating_sub(display_width(&directory_prefix));

        let lines = vec![
            Line::from(vec![Span::from(">_ "), Span::from("RARA")]),
            Line::from(""),
            Line::from(vec![
                Span::from(model_prefix),
                Span::from(model_value),
                Span::from(" ".repeat(gap_width)),
                Span::from(hint),
            ]),
            Line::from(vec![
                Span::from(directory_prefix),
                Span::from(truncate_path_middle(&self.directory, directory_max_width)),
            ]),
        ];

        with_border(lines, inner_width)
    }
}

pub(super) fn planning_suggestion_text(app: &TuiApp) -> String {
    status_planning_suggestion_text(app)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::{HistoryCell, SummaryCell};
    use crate::tui::theme::PHASE_RAN;

    #[test]
    fn summary_cell_renders_indented_diff_block_as_patch_preview() {
        let cell = SummaryCell::new(
            "Ran",
            PHASE_RAN,
            "  replace src/main.rs\n  diff:\n  *** Begin Patch\n  *** Update File: src/main.rs\n  @@\n  -old\n  +new\n  *** End Patch",
        );

        let rendered = cell
            .display_lines(100)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("diff:"));
        assert!(rendered.contains("Edited src/main.rs"));
        assert!(rendered.contains("- old"));
        assert!(rendered.contains("+ new"));
    }
}
