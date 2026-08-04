use ratatui::{style::Color, text::Line};

use crate::tui::state::{TranscriptEntry, TranscriptEntryPayload};

mod active_turn;
mod committed_turn;
mod compaction;
mod interaction_cells;
mod lsp_diagnostics;
mod message_cell;
mod plan_cells;
mod responding_cell;
mod summary_cells;
mod thinking_cells;
mod tool_progress;
mod user_startup;

pub(crate) use self::active_turn::ActiveTurnCell;
pub(crate) use self::committed_turn::CommittedTurnCell;
pub(crate) use self::compaction::CompactionCell;
pub(crate) use self::interaction_cells::{
    CommittedInteractionCell, PendingInteractionCell, QueuedFollowUpCell, TerminalCell,
};
pub(crate) use self::lsp_diagnostics::LspDiagnosticsCell;
pub(crate) use self::message_cell::MessageCell;
pub(crate) use self::plan_cells::{
    PlanModeCell, PlanSummaryCell, PlanningSuggestionCell, planning_suggestion_text,
};
pub(crate) use self::responding_cell::RespondingCell;
pub(crate) use self::summary_cells::{ExploringCell, PlanningCell, RunningCell};
pub(crate) use self::thinking_cells::ThinkingBlockCell;
pub(crate) use self::user_startup::{StartupCardCell, UserCell};

pub(crate) trait HistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;
}

pub(crate) trait ActiveCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;
}

enum OrderedActiveSegment<'a> {
    Exploration(Vec<String>),
    Progress(ProgressRole, Vec<String>),
    Agent(&'a str),
}

struct TerminalCellData {
    command: String,
    output: Vec<String>,
    output_deltas: Vec<(crate::tui::terminal_event::TerminalStream, String)>,
    active: bool,
    success: Option<bool>,
}

pub(super) fn trim_trailing_empty_lines(lines: &mut Vec<Line<'static>>) {
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

pub(super) fn is_progress_stack_title(line: &Line<'static>) -> bool {
    matches!(
        line_plain_text(line).trim(),
        "Plan Mode" | "Thinking" | "Exploring" | "Planning" | "Running"
    )
}

pub(super) mod progress;
use self::progress::ProgressRole;
pub(super) mod plan;
pub(super) mod terminal;

fn ordered_exploration_agent_segments<'a>(
    current_turn: &[&'a TranscriptEntry],
) -> Option<Vec<OrderedActiveSegment<'a>>> {
    let mut segments = Vec::new();
    let mut exploration_items = Vec::new();
    let mut saw_interleaving = false;

    let flush_exploration = |segments: &mut Vec<OrderedActiveSegment<'a>>,
                             items: &mut Vec<String>| {
        if !items.is_empty() {
            segments.push(OrderedActiveSegment::Exploration(std::mem::take(items)));
        }
    };

    for entry in current_turn {
        match entry.role.as_str() {
            "Tool" => {
                if let Some(action) = super::exploration_action_label(&entry.message) {
                    exploration_items.push(action);
                }
            }
            "Exploring" => {
                for item in entry
                    .message
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| {
                        line.trim_start_matches("└")
                            .trim_start_matches("•")
                            .trim()
                            .to_string()
                    })
                    .filter(|line| !line.is_empty())
                {
                    exploration_items.push(item);
                }
            }
            role if let Some(progress_role) = ProgressRole::from_entry_role(role) => {
                let messages =
                    progress::progress_entry_message_lines(progress_role, &entry.message);
                if messages.is_empty() {
                    continue;
                }
                if !exploration_items.is_empty() || !segments.is_empty() {
                    saw_interleaving = true;
                }
                flush_exploration(&mut segments, &mut exploration_items);
                if let Some(OrderedActiveSegment::Progress(last_role, last_messages)) =
                    segments.last_mut()
                    && *last_role == progress_role
                {
                    last_messages.extend(messages);
                } else {
                    segments.push(OrderedActiveSegment::Progress(progress_role, messages));
                }
            }
            "Agent" => {
                if !exploration_items.is_empty() {
                    saw_interleaving = true;
                    flush_exploration(&mut segments, &mut exploration_items);
                }
                segments.push(OrderedActiveSegment::Agent(entry.message.as_str()));
            }
            "Tool Result" | "Tool Error" | "Tool Progress" | "System"
                if !exploration_items.is_empty() =>
            {
                saw_interleaving = true;
                flush_exploration(&mut segments, &mut exploration_items);
            }
            _ => {}
        }
    }

    flush_exploration(&mut segments, &mut exploration_items);

    let simple_exploration_then_agent = segments.len() == 2
        && matches!(segments.first(), Some(OrderedActiveSegment::Exploration(_)))
        && matches!(segments.last(), Some(OrderedActiveSegment::Agent(_)));

    if saw_interleaving || (segments.len() > 1 && !simple_exploration_then_agent) {
        Some(segments)
    } else {
        None
    }
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
    fn from_role(role: &str) -> Option<Self> {
        match role {
            "Shell Approval Completed" => Some(Self::ShellApprovalCompleted),
            "Question Answered" => Some(Self::QuestionAnswered),
            "Planning Question Answered" => Some(Self::PlanningQuestionAnswered),
            "Exploration Question Answered" => Some(Self::ExplorationQuestionAnswered),
            "Sub-agent Question Answered" => Some(Self::SubAgentQuestionAnswered),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::ShellApprovalCompleted => "Shell Approval Completed",
            Self::QuestionAnswered => "Question Answered",
            Self::PlanningQuestionAnswered => "Planning Question Answered",
            Self::ExplorationQuestionAnswered => "Exploration Question Answered",
            Self::SubAgentQuestionAnswered => "Sub-agent Question Answered",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::ShellApprovalCompleted
            | Self::QuestionAnswered
            | Self::PlanningQuestionAnswered
            | Self::ExplorationQuestionAnswered
            | Self::SubAgentQuestionAnswered => Color::LightGreen,
        }
    }
}

pub(super) fn completion_role_kind(role: &str) -> Option<InteractionCompletionKind> {
    InteractionCompletionKind::from_role(role)
}

#[cfg(test)]
mod helper_tests {
    use super::{
        HistoryCell, RespondingCell, plan::compact_live_response_message,
        plan::compact_live_response_source, plan::parse_render_plan_block,
    };

    #[test]
    fn compact_live_response_message_keeps_markdown_source_intact() {
        let rendered = compact_live_response_message(
            "Let me trace `AnalyzeExec.Next()` including `MemTracker.AttachTo(GlobalAnalyzeMemoryTracker)`. Next I will inspect `select.go`.",
        )
        .unwrap();

        assert_eq!(
            rendered,
            "Let me trace `AnalyzeExec.Next()` including `MemTracker.AttachTo(GlobalAnalyzeMemoryTracker)`.\nNext I will inspect `select.go`."
        );
    }

    #[test]
    fn compact_live_response_message_prefers_first_sentence_and_next_step() {
        let rendered = compact_live_response_message(
            "I inspected the repository structure. I checked the runtime boundary. I checked the prompt assembly path. Next I will inspect the persistence layer. Then I will verify the restore contract.",
        )
        .unwrap();

        assert_eq!(
            rendered,
            "I inspected the repository structure.\nI checked the runtime boundary.\nNext I will inspect the persistence layer."
        );
    }

    #[test]
    fn compact_responding_cell_preserves_line_breaks_and_inline_code() {
        let lines = RespondingCell::from_compact_message(
            "Inspect `AnalyzeExec.Next()`.\nNext I will inspect `select.go`.".to_string(),
            4,
            None,
        )
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                "• Inspect AnalyzeExec.Next().".to_string(),
                "• Next I will inspect select.go.".to_string(),
            ]
        );
    }

    #[test]
    fn compact_live_response_source_strips_structured_plan_block() {
        let rendered = compact_live_response_source(
            "我先对比代码结构和现有 todo，找出未记录但值得改进的点。\n我再补两处证据，尽量把建议落到具体代码位置。\n<proposed_plan>\n- [completed] Inspect runtime entrypoint\n- [pending] Tighten render path\n</proposed_plan>",
        )
        .unwrap();

        assert_eq!(
            rendered,
            "我先对比代码结构和现有 todo，找出未记录但值得改进的点。\n我再补两处证据，尽量把建议落到具体代码位置。"
        );
    }

    #[test]
    fn compact_live_response_source_drops_checklist_tail_after_prose() {
        let rendered = compact_live_response_source(
            "I inspected the current context path.\nI will reuse the existing assembler output.\n- [completed] Review context/runtime.rs\n- [pending] Add a focused test",
        )
        .unwrap();

        assert_eq!(
            rendered,
            "I inspected the current context path.\nI will reuse the existing assembler output."
        );
    }

    #[test]
    fn compact_live_response_source_keeps_prose_after_structured_plan_block() {
        let rendered = compact_live_response_source(
            "I inspected the current context path.\n<proposed_plan>\n- [completed] Review context/runtime.rs\n- [pending] Add a focused test\n</proposed_plan>\nI am starting the focused patch now.",
        )
        .unwrap();

        assert_eq!(
            rendered,
            "I inspected the current context path.\nI am starting the focused patch now."
        );
    }

    #[test]
    fn parse_render_plan_block_extracts_steps_and_explanation() {
        let parsed = parse_render_plan_block(
            "I reviewed the code.\n<proposed_plan>\n- [completed] Inspect the runtime path\n- Tighten the render path\n</proposed_plan>\nKeep the diff narrow.",
        )
        .unwrap();

        assert_eq!(
            parsed,
            (
                vec![
                    (
                        "completed".to_string(),
                        "Inspect the runtime path".to_string()
                    ),
                    ("pending".to_string(), "Tighten the render path".to_string()),
                ],
                Some("Keep the diff narrow.".to_string()),
            )
        );
    }
}

pub(super) fn is_renderable_system_message(entry: &TranscriptEntry) -> bool {
    matches!(entry.payload, Some(TranscriptEntryPayload::System(_)))
}
#[cfg(test)]
#[path = "cells_tests.rs"]
mod tests; // #[path] set above
