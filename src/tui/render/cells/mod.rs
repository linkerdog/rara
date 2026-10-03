mod active_turn;
mod committed_turn;
mod common;
mod compaction;
mod interaction_cells;
mod lsp_diagnostics;
mod message_cell;
mod ordered_segments;
pub(super) mod plan;
mod plan_cells;
pub(super) mod progress;
mod responding_cell;
mod summary_cells;
pub(super) mod terminal;
mod thinking_cells;
mod tool_progress;
mod user_startup;

pub(crate) use self::active_turn::ActiveTurnCell;
pub(crate) use self::committed_turn::CommittedTurnCell;
use self::common::TerminalCellData;
pub(crate) use self::common::{HistoryCell, InteractionCompletionKind};
pub(super) use self::common::{
    completion_role_kind, is_progress_stack_title, is_renderable_system_message,
    trim_trailing_empty_lines,
};
pub(crate) use self::compaction::CompactionCell;
pub(crate) use self::interaction_cells::{
    CommittedInteractionCell, PendingInteractionCell, QueuedFollowUpCell, TerminalCell,
};
pub(crate) use self::lsp_diagnostics::LspDiagnosticsCell;
pub(crate) use self::message_cell::MessageCell;
use self::ordered_segments::{OrderedActiveSegment, ordered_exploration_agent_segments};
pub(crate) use self::plan_cells::{
    PlanModeCell, PlanSummaryCell, PlanningSuggestionCell, planning_suggestion_text,
};
pub(crate) use self::responding_cell::RespondingCell;
pub(crate) use self::summary_cells::{ExploringCell, PlanningCell, RunningCell};
pub(crate) use self::thinking_cells::ThinkingBlockCell;
pub(crate) use self::user_startup::{StartupCardCell, UserCell};

#[cfg(test)]
mod helper_tests;
#[cfg(test)]
#[path = "cells_tests.rs"]
mod tests;
