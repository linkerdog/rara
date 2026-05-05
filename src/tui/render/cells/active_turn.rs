use std::path::Path;

use ratatui::text::Line;

use super::components::{
    CommittedInteractionCell, ExploringCell, MessageCell, PendingInteractionCell,
    PlanModeCell, PlanSummaryCell, PlanningCell, PlanningSuggestionCell, QueuedFollowUpCell, RespondingCell, RunningCell,
    UserCell, planning_suggestion_text,
};
use super::plan::{compact_live_response_message, parse_render_plan_block};
use super::progress::{
    ProgressRole, explicit_progress_entry_groups, push_live_events, push_progress_group,
};
use super::terminal::terminal_cell_from_entries;
use super::{
    ActiveCell, HistoryCell, InteractionCompletionKind, OrderedActiveSegment,
    completion_role_kind, is_progress_stack_title, is_renderable_system_message,
    ordered_exploration_agent_segments, trim_trailing_empty_lines,
};
use crate::tui::interaction_text::{
    pending_interaction_detail_text, pending_interaction_shortcut_text,
};
use crate::tui::plan_display::should_show_updated_plan;
use crate::tui::queued_input::queued_follow_up_sections;
use crate::tui::render::{
    compact_progress_summary_lines, compact_recent_first_summary_lines, compact_summary_lines,
    compact_summary_text, current_turn_exploration_summary_from_entries, current_turn_tool_summary,
};
use crate::tui::state::{
    RuntimePhase, TranscriptEntryPayload, TuiApp,
    contains_structured_planning_output,
};

pub(crate) struct ActiveTurnCell<'a> {
    app: &'a TuiApp,
    cwd: Option<&'a Path>,
}

impl<'a> ActiveTurnCell<'a> {
    pub(crate) fn new(app: &'a TuiApp, cwd: Option<&'a Path>) -> Self {
        Self { app, cwd }
    }
}

impl ActiveCell for ActiveTurnCell<'_> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let current_turn = self.app.active_turn.entries.iter().collect::<Vec<_>>();
        let turn_live = self.app.is_busy()
            || matches!(
                self.app.runtime_phase,
                RuntimePhase::SendingPrompt
                    | RuntimePhase::ProcessingResponse
                    | RuntimePhase::RunningTool
            );
        if current_turn.is_empty() {
            if let Some(prompt) = self.app.pending_planning_suggestion.as_deref() {
                let cells: Vec<Box<dyn HistoryCell + '_>> = vec![
                    Box::new(UserCell::new(prompt)),
                    Box::new(PlanningSuggestionCell::new(planning_suggestion_text(
                        self.app,
                    ))),
                ];
                let mut lines = Vec::new();
                for (idx, cell) in cells.into_iter().enumerate() {
                    if idx > 0 {
                        lines.push(Line::from(""));
                    }
                    lines.extend(cell.display_lines(width));
                }
                trim_trailing_empty_lines(&mut lines);
                return lines;
            }
            if turn_live {
                let has_pending_surface = self.app.active_pending_interaction().is_some()
                    || self.app.has_queued_follow_up_messages()
                    || self.app.has_pending_planning_suggestion();
                if has_pending_surface {
                    // Continue through the normal section assembly so resumed approval
                    // and request-input turns can render their actionable cards even
                    // before the first transcript entry arrives.
                } else {
                    return RespondingCell::working(
                        self.app
                            .runtime_phase_detail
                            .as_deref()
                            .unwrap_or("waiting for the current turn to finish"),
                    )
                    .display_lines(width);
                }
            }
            if !turn_live {
                return Vec::new();
            }
        }
        let has_tool_activity = current_turn.iter().any(|entry| {
            matches!(
                entry.role.as_str(),
                "Tool" | "Tool Result" | "Tool Error" | "Tool Progress"
            ) || matches!(entry.payload, Some(TranscriptEntryPayload::Terminal(_)))
        });
        let user_message = current_turn
            .iter()
            .find(|entry| entry.role == "You")
            .map(|entry| entry.message.as_str())
            .unwrap_or("");
        let latest_agent = current_turn
            .iter()
            .rev()
            .find(|entry| entry.role == "Agent")
            .map(|entry| entry.message.as_str());
        let streaming_agent_lines = self.app.agent_stream_lines();
        let has_agent_stream = self.app.has_agent_stream();
        let streaming_thinking_lines = self.app.agent_thinking_stream_lines();
        let has_thinking_stream = self.app.has_agent_thinking_stream();
        let latest_system = current_turn
            .iter()
            .rev()
            .find(|entry| {
                entry.role == "System" && is_renderable_system_message(entry.message.as_str())
            })
            .map(|entry| entry.message.as_str());
        let latest_tool_result = current_turn
            .iter()
            .rev()
            .find(|entry| {
                matches!(
                    entry.role.as_str(),
                    "Tool Result" | "Tool Error" | "Tool Progress"
                )
            })
            .map(|entry| (entry.role.as_str(), entry.message.as_str()));
        let latest_completion = current_turn.iter().rev().find(|entry| {
            let Some(kind) = completion_role_kind(entry.role.as_str()) else {
                return false;
            };
            !(turn_live && matches!(kind, InteractionCompletionKind::ShellApprovalCompleted))
        });
        let mut cells: Vec<Box<dyn HistoryCell + '_>> = Vec::new();
        let has_live_exploration = !self.app.active_live.exploration_actions.is_empty()
            || !self.app.active_live.exploration_notes.is_empty();
        let has_live_planning = !self.app.active_live.planning_actions.is_empty()
            || !self.app.active_live.planning_notes.is_empty();
        let has_live_running = !self.app.active_live.running_actions.is_empty();
        let live_events = self.app.active_live.events.as_slice();
        let has_live_events = !live_events.is_empty();
        let has_active_pending_interaction = self.app.active_pending_interaction().is_some();

        if !user_message.is_empty() {
            cells.push(Box::new(UserCell::new(user_message)));
        }

        if self.app.agent_execution_mode_label() == "plan" && !self.app.has_pending_plan_approval()
        {
            cells.push(Box::new(PlanModeCell));
        }

        let ordered_exploration_agent_segments =
            if !has_live_events && !has_live_exploration && !has_live_planning && !has_live_running
            {
                ordered_exploration_agent_segments(current_turn.as_slice())
            } else {
                None
            };
        let uses_ordered_exploration_agent_segments = ordered_exploration_agent_segments.is_some();

        if let Some(segments) = ordered_exploration_agent_segments.as_ref() {
            for segment in segments {
                match segment {
                    OrderedActiveSegment::Exploration(items) => {
                        let summary =
                            compact_summary_lines(items.as_slice(), 4, "more exploration step(s)");
                        cells.push(Box::new(ExploringCell::new(summary, turn_live)));
                    }
                    OrderedActiveSegment::Agent(message) => {
                        cells.push(Box::new(MessageCell::new(
                            "Agent",
                            message,
                            usize::MAX,
                            self.cwd,
                        )));
                    }
                }
            }
        }

        let mut has_event_exploration_summary = false;
        let mut has_event_planning_summary = false;
        let mut has_event_running_summary = false;
        let has_live_thinking = turn_live && has_thinking_stream;
        if has_live_events || has_live_thinking {
            for event in live_events {
                match ProgressRole::from_live_event(event) {
                    ProgressRole::Exploring => has_event_exploration_summary = true,
                    ProgressRole::Planning => has_event_planning_summary = true,
                    ProgressRole::Running => has_event_running_summary = true,
                    ProgressRole::Thinking => {}
                }
            }
            push_live_events(
                &mut cells,
                live_events,
                streaming_thinking_lines.filter(|_| has_live_thinking),
                true,
            );
        }

        let explicit_progress_groups =
            (!has_live_events && !has_live_thinking && !has_active_pending_interaction)
                .then(|| explicit_progress_entry_groups(current_turn.iter().copied()));
        if let Some(groups) = explicit_progress_groups.as_ref() {
            for (role, messages) in groups {
                push_progress_group(&mut cells, *role, messages.clone(), turn_live);
            }
        }
        let has_explicit_progress_groups = explicit_progress_groups
            .as_ref()
            .is_some_and(|groups| !groups.is_empty());

        let explicit_exploration = current_turn
            .iter()
            .find(|entry| entry.role == "Exploring")
            .map(|entry| entry.message.clone());

        let exploration_summary = if has_live_events
            || has_explicit_progress_groups
            || uses_ordered_exploration_agent_segments
        {
            None
        } else if has_live_exploration {
            Some(compact_progress_summary_lines(
                self.app.active_live.exploration_actions.as_slice(),
                self.app.active_live.exploration_notes.as_slice(),
                4,
                "more exploration step(s)",
            ))
        } else if !turn_live {
            None
        } else {
            explicit_exploration
                .map(|summary| compact_summary_text(&summary, 4, "more exploration step(s)"))
                .or_else(|| {
                    current_turn_exploration_summary_from_entries(
                        current_turn.as_slice(),
                        self.app.is_busy() && turn_live,
                        self.app.runtime_phase_detail.as_deref(),
                    )
                })
        };
        let has_exploration_summary = exploration_summary.is_some();
        let exploration_active = turn_live && has_exploration_summary;
        if let Some(summary) = exploration_summary {
            cells.push(Box::new(ExploringCell::new(summary, exploration_active)));
        }

        let explicit_planning = current_turn
            .iter()
            .find(|entry| entry.role == "Planning")
            .map(|entry| entry.message.clone());

        let planning_summary = if has_live_events || has_explicit_progress_groups {
            None
        } else if has_live_planning {
            Some(compact_progress_summary_lines(
                self.app.active_live.planning_actions.as_slice(),
                self.app.active_live.planning_notes.as_slice(),
                4,
                "more planning step(s)",
            ))
        } else {
            explicit_planning
                .map(|summary| compact_summary_text(&summary, 4, "more planning step(s)"))
        };
        let has_planning_summary = planning_summary.is_some();
        if let Some(summary) = planning_summary {
            cells.push(Box::new(PlanningCell::new(summary, turn_live)));
        }

        let explicit_running = current_turn
            .iter()
            .find(|entry| entry.role == "Running")
            .map(|entry| entry.message.clone());

        let running_summary = if has_live_events || has_explicit_progress_groups {
            None
        } else if has_live_running {
            Some(compact_recent_first_summary_lines(
                self.app.active_live.running_actions.as_slice(),
                4,
                "more running step(s)",
            ))
        } else {
            explicit_running
                .map(|summary| compact_summary_text(&summary, 4, "more running step(s)"))
                .or_else(|| {
                    current_turn_tool_summary(
                        current_turn.as_slice(),
                        turn_live,
                        self.app.runtime_phase_detail.as_deref(),
                    )
                })
        };
        let has_running_summary = running_summary.is_some();
        let running_active = turn_live && has_running_summary;
        if let Some(cell) = terminal_cell_from_entries(current_turn.iter().copied()) {
            cells.push(Box::new(cell));
        } else if let Some(summary) = running_summary {
            cells.push(Box::new(RunningCell::new(summary, running_active)));
        }
        let compact_live_response = turn_live
            && (has_exploration_summary
                || has_planning_summary
                || has_running_summary
                || has_event_exploration_summary
                || has_event_planning_summary
                || has_event_running_summary);

        let inline_plan_summary = latest_agent
            .and_then(|message| parse_render_plan_block(message))
            .filter(|_| {
                self.app.snapshot.plan_steps.is_empty()
                    && matches!(
                        self.app.agent_execution_mode,
                        crate::agent::AgentExecutionMode::Plan
                    )
            });

        if should_show_updated_plan(self.app) {
            cells.push(Box::new(PlanSummaryCell::new(
                self.app.snapshot.plan_steps.clone(),
                self.app.snapshot.plan_explanation.clone(),
            )));
        } else if let Some((steps, explanation)) = inline_plan_summary.clone() {
            cells.push(Box::new(PlanSummaryCell::new(steps, explanation)));
        }

        if let Some(pending) = self.app.active_pending_interaction() {
            let mut request_lines = pending_interaction_detail_text(self.app, pending.kind)
                .lines()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            request_lines.push(pending_interaction_shortcut_text(pending.kind).to_string());
            cells.push(Box::new(PendingInteractionCell::new(
                pending.kind,
                request_lines,
            )));
        }

        let queued_sections = queued_follow_up_sections(
            self.app.pending_follow_up_preview(),
            self.app.pending_follow_up_count(),
            self.app.queued_end_of_turn_preview(),
            self.app.queued_follow_up_messages.len(),
        );
        if !queued_sections.is_empty() {
            cells.push(Box::new(QueuedFollowUpCell::new(queued_sections)));
        }

        if self.app.pending_planning_suggestion.is_some() {
            cells.push(Box::new(PlanningSuggestionCell::new(
                planning_suggestion_text(self.app),
            )));
        }

        if let Some(entry) = latest_completion {
            if let Some(kind) = completion_role_kind(entry.role.as_str()) {
                cells.push(Box::new(CommittedInteractionCell::new(
                    kind,
                    entry.message.clone(),
                )));
            }
        }

        let suppress_intermediate_agent = turn_live
            && has_tool_activity
            && matches!(
                self.app.runtime_phase,
                RuntimePhase::RunningTool | RuntimePhase::SendingPrompt
            );
        let suppress_planning_chatter = matches!(
            self.app.agent_execution_mode,
            crate::agent::AgentExecutionMode::Plan
        ) && has_exploration_summary
            && latest_agent.is_some_and(|message| !contains_structured_planning_output(message))
            && self.app.snapshot.plan_steps.is_empty()
            && self.app.pending_request_input().is_none()
            && !self.app.has_pending_plan_approval();
        let suppress_structured_plan_response = (self.app.snapshot.plan_steps.is_empty()
            && inline_plan_summary.is_some())
            || (!self.app.snapshot.plan_steps.is_empty()
                && latest_agent.is_some_and(contains_structured_planning_output));

        let responding_role = if turn_live { "Responding" } else { "Agent" };
        let prefer_responding_chrome = turn_live
            && matches!(
                self.app.runtime_phase,
                RuntimePhase::SendingPrompt | RuntimePhase::ProcessingResponse
            )
            && !has_exploration_summary
            && !has_planning_summary
            && !has_running_summary
            && !has_event_exploration_summary
            && !has_event_planning_summary
            && !has_event_running_summary
            && self.app.snapshot.plan_steps.is_empty()
            && !suppress_planning_chatter
            && !suppress_structured_plan_response
            && self.app.pending_request_input().is_none()
            && !self.app.has_pending_plan_approval()
            && self.app.pending_command_approval().is_none()
            && self.app.pending_planning_suggestion.is_none();

        if uses_ordered_exploration_agent_segments {
            // Preserve chronological "explore -> agent -> explore" segments without
            // reintroducing the latest agent/tool fallback below.
        } else if has_agent_stream
            && !suppress_intermediate_agent
            && !suppress_planning_chatter
            && !suppress_structured_plan_response
        {
            if let Some(stream_lines) = streaming_agent_lines {
                if compact_live_response {
                    cells.push(Box::new(RespondingCell::from_stream_compact(
                        stream_lines,
                        4,
                    )));
                } else {
                    cells.push(Box::new(RespondingCell::from_stream(stream_lines)));
                }
            } else if let Some(agent_message) = latest_agent {
                if compact_live_response {
                    if let Some(message) = compact_live_response_message(agent_message) {
                        cells.push(Box::new(RespondingCell::from_compact_message(
                            message,
                            usize::MAX,
                        )));
                    }
                } else {
                    cells.push(Box::new(RespondingCell::from_message(
                        responding_role,
                        agent_message,
                        usize::MAX,
                        self.cwd,
                    )));
                }
            } else {
                cells.push(Box::new(RespondingCell::working(
                    self.app
                        .runtime_phase_detail
                        .as_deref()
                        .unwrap_or("streaming model output"),
                )));
            }
        } else if let Some(agent_message) = latest_agent.filter(|_| {
            !suppress_intermediate_agent
                && !suppress_planning_chatter
                && !suppress_structured_plan_response
        }) {
            if compact_live_response {
                if let Some(message) = compact_live_response_message(agent_message) {
                    cells.push(Box::new(RespondingCell::from_compact_message(
                        message,
                        usize::MAX,
                    )));
                }
            } else {
                cells.push(Box::new(RespondingCell::from_message(
                    responding_role,
                    agent_message,
                    usize::MAX,
                    self.cwd,
                )));
            }
        } else if prefer_responding_chrome {
            cells.push(Box::new(RespondingCell::working(
                self.app
                    .runtime_phase_detail
                    .as_deref()
                    .unwrap_or("waiting for model output"),
            )));
        } else if let Some(system_message) = latest_system {
            cells.push(Box::new(RespondingCell::from_message(
                "System",
                system_message,
                14,
                self.cwd,
            )));
        } else if !has_active_pending_interaction
            && let Some((role, tool_result)) = latest_tool_result
        {
            cells.push(Box::new(RespondingCell::from_tool_result(
                role,
                tool_result,
                14,
            )));
        } else if turn_live
            && !has_exploration_summary
            && !has_planning_summary
            && !has_running_summary
            && !has_event_exploration_summary
            && !has_event_planning_summary
            && !has_event_running_summary
            && self.app.pending_request_input().is_none()
            && !self.app.has_pending_plan_approval()
            && self.app.pending_command_approval().is_none()
            && self.app.pending_planning_suggestion.is_none()
            && self.app.snapshot.plan_steps.is_empty()
        {
            cells.push(Box::new(RespondingCell::working(
                self.app
                    .runtime_phase_detail
                    .as_deref()
                    .unwrap_or("waiting for the current turn to finish"),
            )));
        }

        let mut lines = Vec::new();
        let mut previous_was_progress_stack_title = false;
        for (idx, cell) in cells.into_iter().enumerate() {
            let cell_lines = cell.display_lines(width);
            let current_is_progress_stack_title =
                cell_lines.first().is_some_and(is_progress_stack_title);
            if idx > 0 && !(previous_was_progress_stack_title && current_is_progress_stack_title) {
                lines.push(Line::from(""));
            }
            lines.extend(cell_lines);
            previous_was_progress_stack_title = current_is_progress_stack_title;
        }

        trim_trailing_empty_lines(&mut lines);
        lines
    }
}
