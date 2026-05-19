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
use crate::tui::terminal_event::TerminalStream;
use crate::tui::theme::*;

pub(crate) struct TerminalCell {
    command: String,
    output: Vec<String>,
    output_deltas: Vec<(TerminalStream, String)>,
    active: bool,
    success: Option<bool>,
}

impl TerminalCell {
    pub(crate) fn new(
        command: impl Into<String>,
        output: Vec<String>,
        output_deltas: Vec<(TerminalStream, String)>,
        active: bool,
        success: Option<bool>,
    ) -> Self {
        Self {
            command: command.into(),
            output,
            output_deltas,
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

        // Live output delta section (shown while terminal is running)
        if self.active && !self.output_deltas.is_empty() {
            let live_lines: Vec<String> = self
                .output_deltas
                .iter()
                .flat_map(|(stream, chunk)| {
                    let prefix = match stream {
                        TerminalStream::Stderr => "[stderr] ",
                        TerminalStream::Stdout => "",
                    };
                    chunk.lines().map(move |line| format!("{prefix}{line}"))
                })
                .collect();
            if !live_lines.is_empty() {
                lines.extend(Self::fold_output(live_lines.len(), &live_lines, "    "));
            }
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
        // Deliberately no special background: the card content (command output, etc.)
        // renders on the main terminal background so text is always readable.
        // The section label provides the only visual framing.
        let card_style = Style::default().fg(PENDING_CARD_FG);
        let mut lines = vec![Line::from(section_label(self.title, self.color))];
        lines.extend(
            self.lines
                .iter()
                .map(|line| Line::from(Span::styled(format!("  {line}"), card_style))),
        );
        lines
    }
}

pub(crate) struct PendingInteractionCell {
    inner: ApprovalCell,
}

impl PendingInteractionCell {
    pub(crate) fn new(kind: ActivePendingInteractionKind, lines: Vec<String>) -> Self {
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

pub(crate) struct PlanningSuggestionCell {
    text: String,
}

impl PlanningSuggestionCell {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl HistoryCell for PlanningSuggestionCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(section_label(
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

pub(crate) struct QueuedFollowUpCell {
    sections: Vec<QueuedFollowUpSection>,
}

impl QueuedFollowUpCell {
    pub(crate) fn new(sections: Vec<QueuedFollowUpSection>) -> Self {
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
                    section_label("Queued", PHASE_PLANNING),
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

pub(crate) struct CommittedInteractionCell {
    inner: CompletionCell,
}

impl CommittedInteractionCell {
    pub(crate) fn new(kind: InteractionCompletionKind, summary: impl Into<String>) -> Self {
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
            Line::from(section_label(&self.title, self.color)),
            Line::from(format!("  {}", self.summary)),
        ]
    }
}
