use ratatui::{style::Color, text::Line};

use crate::tui::message_role::MessageRole;
use crate::tui::state::{TranscriptEntry, TranscriptEntryPayload};

/// Render a cell without changing its source content or presentation state.
pub(crate) trait HistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;
}

pub(super) struct TerminalCellData {
    pub(super) command: String,
    pub(super) output: Vec<String>,
    pub(super) output_deltas: Vec<(crate::tui::terminal_event::TerminalStream, String)>,
    pub(super) active: bool,
    pub(super) success: Option<bool>,
}

pub(crate) fn trim_trailing_empty_lines(lines: &mut Vec<Line<'static>>) {
    while matches!(lines.last(), Some(line) if line.spans.iter().all(|span| span.content.is_empty()))
    {
        lines.pop();
    }
}

fn line_plain_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

pub(crate) fn is_progress_stack_title(line: &Line<'static>) -> bool {
    matches!(
        line_plain_text(line).trim(),
        "Plan Mode" | "Thinking" | "Exploring" | "Planning" | "Running"
    )
}

#[derive(Clone, Copy)]
pub(crate) enum InteractionCompletionKind {
    ShellApprovalCompleted,
    QuestionAnswered,
    PlanningQuestionAnswered,
    ExplorationQuestionAnswered,
    SubAgentQuestionAnswered,
}

impl InteractionCompletionKind {
    pub(super) fn from_role(role: &MessageRole) -> Option<Self> {
        match role {
            MessageRole::ShellApprovalCompleted => Some(Self::ShellApprovalCompleted),
            MessageRole::QuestionAnswered => Some(Self::QuestionAnswered),
            MessageRole::PlanningQuestionAnswered => Some(Self::PlanningQuestionAnswered),
            MessageRole::ExplorationQuestionAnswered => Some(Self::ExplorationQuestionAnswered),
            MessageRole::SubAgentQuestionAnswered => Some(Self::SubAgentQuestionAnswered),
            MessageRole::User
            | MessageRole::Agent
            | MessageRole::System
            | MessageRole::Runtime
            | MessageRole::Responding
            | MessageRole::Tool
            | MessageRole::ToolResult
            | MessageRole::ToolError
            | MessageRole::ToolProgress
            | MessageRole::Exploring
            | MessageRole::Planning
            | MessageRole::Running
            | MessageRole::Thinking
            | MessageRole::Todo
            | MessageRole::Download
            | MessageRole::TerminalEvent
            | MessageRole::Compaction
            | MessageRole::PlanDecision
            | MessageRole::Legacy(_) => None,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::ShellApprovalCompleted => "Shell Approval Completed",
            Self::QuestionAnswered => "Question Answered",
            Self::PlanningQuestionAnswered => "Planning Question Answered",
            Self::ExplorationQuestionAnswered => "Exploration Question Answered",
            Self::SubAgentQuestionAnswered => "Sub-agent Question Answered",
        }
    }

    pub(super) fn color(self) -> Color {
        match self {
            Self::ShellApprovalCompleted
            | Self::QuestionAnswered
            | Self::PlanningQuestionAnswered
            | Self::ExplorationQuestionAnswered
            | Self::SubAgentQuestionAnswered => Color::LightGreen,
        }
    }
}

pub(crate) fn completion_role_kind(role: &MessageRole) -> Option<InteractionCompletionKind> {
    InteractionCompletionKind::from_role(role)
}

pub(crate) fn is_renderable_system_message(entry: &TranscriptEntry) -> bool {
    matches!(entry.payload, Some(TranscriptEntryPayload::System(_)))
}
