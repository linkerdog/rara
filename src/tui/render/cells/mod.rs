use ratatui::{style::Color, text::Line};

use crate::tui::state::TranscriptEntry;

#[path = "active_turn.rs"]
mod active_turn;
#[path = "committed_turn.rs"]
mod committed_turn;
#[path = "cells_components.rs"]
mod components;
mod tool_progress;

pub(crate) use self::active_turn::ActiveTurnCell;
pub(crate) use self::components::StartupCardCell;
use super::wrapped_history_line_count;

pub(crate) trait HistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

    fn desired_height(&self, width: u16) -> u16 {
        wrapped_history_line_count(self.display_lines(width).as_slice(), width)
    }
}

pub(crate) trait ActiveCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;
}

pub(super) enum OrderedActiveSegment<'a> {
    Exploration(Vec<String>),
    Agent(&'a str),
}

struct TerminalCellData {
    command: String,
    output: Vec<String>,
    active: bool,
    success: Option<bool>,
}

pub(super) fn trim_trailing_empty_lines(lines: &mut Vec<Line<'static>>) {
    while matches!(lines.last(), Some(line) if line.spans.iter().all(|span| span.content == "")) {
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
            "Agent" => {
                if !exploration_items.is_empty() {
                    saw_interleaving = true;
                    flush_exploration(&mut segments, &mut exploration_items);
                }
                segments.push(OrderedActiveSegment::Agent(entry.message.as_str()));
            }
            role if ProgressRole::from_entry_role(role).is_some()
                || matches!(
                    role,
                    "Tool Result" | "Tool Error" | "Tool Progress" | "System"
                ) =>
            {
                if !exploration_items.is_empty() {
                    saw_interleaving = true;
                    flush_exploration(&mut segments, &mut exploration_items);
                }
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
pub(super) enum InteractionCompletionKind {
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

pub(crate) use self::committed_turn::CommittedTurnCell;

#[cfg(test)]
mod helper_tests {
    use super::{
        compact_live_response_message, compact_live_response_source, parse_render_plan_block,
        split_progress_sentences,
    };

    #[test]
    fn split_progress_sentences_keeps_ellipses_and_decimal_versions() {
        let sentences = split_progress_sentences(
            "Wait... I checked v1.0 parsing. Next I will inspect restore.",
        );

        assert_eq!(
            sentences,
            vec![
                "Wait...".to_string(),
                "I checked v1.0 parsing.".to_string(),
                "Next I will inspect restore.".to_string(),
            ]
        );
    }

    #[test]
    fn compact_live_response_message_preserves_selected_sentence_order() {
        let rendered = compact_live_response_message(
            "Next I will inspect restore. I checked the auth path. I checked the persistence path. Then I will verify chronology.",
        )
        .unwrap();

        assert_eq!(
            rendered,
            [
                "Next I will inspect restore.",
                "I checked the auth path.",
                "Then I will verify chronology.",
            ]
            .join("\n")
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

pub(super) fn is_renderable_system_message(message: &str) -> bool {
    let lower = message.trim().to_ascii_lowercase();
    lower.starts_with("query failed:")
        || lower.starts_with("compaction failed:")
        || lower.starts_with("compact failed:")
        || lower.starts_with("oauth failed:")
        || lower.starts_with("backend rebuild failed:")
        || lower.starts_with("open this url in a browser and enter the one-time code:")
        || lower.starts_with("starting codex browser login.")
        || lower.starts_with("error:")
}

#[cfg(test)]
#[path = "cells_tests.rs"]
mod tests; // #[path] set above
