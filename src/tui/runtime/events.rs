mod helpers;
#[cfg(test)]
mod tests;

use rara_persistence::redaction::redact_secrets;

use self::helpers::{
    append_tool_progress, exploration_action_label, exploration_action_label_for,
    exploration_note_lines, exploration_result_note, format_tool_result, format_tool_use,
    is_exploration_tool_name, is_oauth_prompt_message, planning_action_label,
    planning_action_label_for, planning_note_lines, planning_result_note,
    scrub_internal_control_tokens, subagent_request_input, tool_action_label,
    tool_action_label_for,
};
use super::super::state::{
    RuntimePhase, SystemMessageKind, TuiApp, TuiEvent, contains_structured_planning_output,
};
use crate::agent::AgentEvent;
use crate::memory_notice::{count_label, memory_notice};
use crate::runtime_control::{
    AssistantEvent, MemoryEvent, RuntimeControlEvent, RuntimeEvent, SessionEvent, ToolEvent,
};
use crate::session_promotion::{
    SessionShardPromotionDecision, SessionShardPromotionOutcome, SessionShardPromotionSkipReason,
};
#[cfg(test)]
use crate::todo::format_todo_update;
use crate::tui::display_sanitize::sanitize_display_text;
use crate::tui::terminal_event::{TerminalEvent, TerminalTarget};

const TOOL_PROGRESS_LINE_LIMIT: usize = 16;
const MEMORY_QUERY_PREVIEW_LIMIT: usize = 120;

pub(crate) fn apply_tui_event(app: &mut TuiApp, event: TuiEvent) {
    match event {
        TuiEvent::Runtime(event) => apply_runtime_control_event(app, *event),
        TuiEvent::Transcript { role, message } => {
            if role == "Status" {
                app.set_runtime_phase(
                    RuntimePhase::ProcessingResponse,
                    Some(message.lines().next().unwrap_or(role).trim().to_string()),
                );
                return;
            } else if role == "Agent Delta" {
                app.set_runtime_phase(
                    RuntimePhase::ProcessingResponse,
                    Some("streaming model output".into()),
                );
                app.append_agent_delta(&message);
                return;
            } else if role == "Agent Thinking Delta" {
                app.set_runtime_phase(RuntimePhase::ProcessingResponse, Some("thinking".into()));
                app.append_agent_thinking_delta(&message);
                return;
            } else if role == "Tool" || role == "Tool Result" || role == "Tool Error" {
                app.finalize_agent_stream(None);
                if role == "Tool" {
                    if let Some(action) = exploration_action_label(&message) {
                        app.record_exploration_action(action);
                    } else if let Some(action) = planning_action_label(&message) {
                        app.record_planning_action(action);
                    } else if let Some(action) = tool_action_label(&message) {
                        app.record_running_action(action);
                    }
                } else if let Some(request) = subagent_request_input(&message) {
                    app.advance_running_tool_boundary();
                    let source = if message.starts_with("explore_agent ") {
                        if let Some(note) = exploration_result_note(&message) {
                            app.record_exploration_note(note);
                        }
                        "explore_agent"
                    } else if message.starts_with("plan_agent ") {
                        if let Some(note) = planning_result_note(&message) {
                            app.record_planning_note(note);
                        }
                        "plan_agent"
                    } else {
                        "spawn_agent"
                    };
                    app.record_local_request_input(
                        source,
                        request.question,
                        request.options,
                        request.note,
                    );
                    app.set_runtime_phase(
                        RuntimePhase::RunningTool,
                        Some(message.lines().next().unwrap_or(role).trim().to_string()),
                    );
                    return;
                } else if let Some(note) = exploration_result_note(&message) {
                    app.advance_running_tool_boundary();
                    app.record_exploration_note(note);
                    app.set_runtime_phase(
                        RuntimePhase::RunningTool,
                        Some(message.lines().next().unwrap_or(role).trim().to_string()),
                    );
                    return;
                } else if let Some(note) = planning_result_note(&message) {
                    app.advance_running_tool_boundary();
                    app.record_planning_note(note);
                    app.set_runtime_phase(
                        RuntimePhase::RunningTool,
                        Some(message.lines().next().unwrap_or(role).trim().to_string()),
                    );
                    return;
                }
                if matches!(role, "Tool Result" | "Tool Error") {
                    app.advance_running_tool_boundary();
                }
                app.set_runtime_phase(
                    RuntimePhase::RunningTool,
                    Some(message.lines().next().unwrap_or(role).trim().to_string()),
                );
            } else if role == "Agent" {
                apply_assistant_text(app, message);
                return;
            } else if role == "Download" {
                let detail = message.lines().next().unwrap_or(role).trim().to_string();
                if detail.starts_with("Ready ·") {
                    app.set_runtime_phase(RuntimePhase::BackendReady, Some(detail));
                } else {
                    app.set_runtime_phase(RuntimePhase::RebuildingBackend, Some(detail));
                }
            } else if role == "Runtime" {
                let detail = message.lines().next().unwrap_or(role).trim().to_string();
                let lower = detail.to_ascii_lowercase();
                if lower.contains("waiting for device-code confirmation")
                    || lower.contains("polling device code")
                {
                    app.set_runtime_phase(RuntimePhase::OAuthPollingDeviceCode, Some(detail));
                } else if is_oauth_prompt_message(&message) {
                    let is_device_code = message.to_ascii_lowercase().contains("one-time code");
                    app.push_system(message, SystemMessageKind::OAuthPrompt);
                    if is_device_code {
                        app.set_runtime_phase(
                            RuntimePhase::OAuthDeviceCodePrompt,
                            Some("device code ready".into()),
                        );
                    } else {
                        app.set_runtime_phase(
                            RuntimePhase::OAuthWaitingCallback,
                            Some("browser login url ready".into()),
                        );
                    }
                    return;
                } else if lower.contains("device-code login")
                    || lower.contains("one-time code")
                    || lower.contains("open this url in a browser")
                    || lower.starts_with("code:")
                {
                    app.set_runtime_phase(RuntimePhase::OAuthDeviceCodePrompt, Some(detail));
                } else if lower.contains("waiting for browser callback") {
                    app.set_runtime_phase(RuntimePhase::OAuthWaitingCallback, Some(detail));
                } else if lower.contains("exchanging token") {
                    app.set_runtime_phase(RuntimePhase::OAuthExchangingToken, Some(detail));
                } else if lower.contains("starting codex browser login")
                    || lower.contains("starting codex browser")
                {
                    app.set_runtime_phase(RuntimePhase::OAuthWaitingCallback, Some(detail));
                } else if lower.contains("starting codex device-code login") {
                    app.set_runtime_phase(RuntimePhase::OAuthDeviceCodePrompt, Some(detail));
                } else {
                    app.set_runtime_phase(RuntimePhase::RebuildingBackend, Some(detail));
                }
            }
            if role == "System" {
                let kind = if message.starts_with("Memory ·") {
                    SystemMessageKind::Memory
                } else {
                    SystemMessageKind::Other
                };
                app.push_system(message, kind)
            } else {
                app.push_entry(role, message)
            }
        }
        TuiEvent::Terminal(TerminalEvent::OutputDelta(event)) => {
            let name = match event.target {
                TerminalTarget::Pty => "pty",
                TerminalTarget::BackgroundTask => "background task",
            };
            if !append_tool_progress(app, name, event.stream.into(), &event.chunk) {
                return;
            }
            app.set_runtime_phase(
                RuntimePhase::RunningTool,
                Some(format!("streaming {name} output")),
            );
        }
        TuiEvent::Terminal(event) => {
            app.finalize_agent_stream(None);
            let role = event.transcript_role();
            let message = event.to_transcript_message();
            if role == "Tool"
                && let Some(action) = tool_action_label(&message)
            {
                app.record_running_action(action);
            }
            if matches!(role, "Tool Result" | "Tool Error") {
                app.advance_running_tool_boundary();
            }
            app.set_runtime_phase(
                RuntimePhase::RunningTool,
                Some(message.lines().next().unwrap_or(role).trim().to_string()),
            );
            app.push_terminal_event(event);
        }
        TuiEvent::ToolProgress {
            name,
            stream,
            chunk,
        } => {
            app.flush_agent_thinking_stream_to_live_event();
            if !append_tool_progress(app, &name, stream, &chunk) {
                return;
            }
            app.set_runtime_phase(
                RuntimePhase::RunningTool,
                Some(format!("streaming {name} output")),
            );
        }
        #[cfg(test)]
        TuiEvent::UpdateTodo(view) => {
            app.snapshot.todo = view;
        }
    }
}

fn apply_runtime_control_event(app: &mut TuiApp, event: RuntimeControlEvent) {
    match event.event {
        RuntimeEvent::Assistant(AssistantEvent::Text(text)) => apply_assistant_text(app, text),
        RuntimeEvent::Assistant(AssistantEvent::TextDelta(text)) => {
            app.set_runtime_phase(
                RuntimePhase::ProcessingResponse,
                Some("streaming model output".into()),
            );
            app.append_agent_delta(&text);
        }
        RuntimeEvent::Assistant(AssistantEvent::ThinkingDelta(text)) => {
            app.set_runtime_phase(RuntimePhase::ProcessingResponse, Some("thinking".into()));
            app.append_agent_thinking_delta(&text);
        }
        RuntimeEvent::Session(SessionEvent::Status { message }) => {
            app.set_runtime_phase(
                RuntimePhase::ProcessingResponse,
                Some(
                    message
                        .lines()
                        .next()
                        .unwrap_or("status")
                        .trim()
                        .to_string(),
                ),
            );
        }
        RuntimeEvent::Session(SessionEvent::TurnStarted)
        | RuntimeEvent::Session(SessionEvent::ModelRequest { .. }) => {}
        RuntimeEvent::Session(SessionEvent::TurnFinished { .. })
        | RuntimeEvent::Session(SessionEvent::TurnCancelled)
        | RuntimeEvent::Session(SessionEvent::TurnInterrupted)
        | RuntimeEvent::Session(SessionEvent::Created { .. })
        | RuntimeEvent::Session(SessionEvent::Resumed { .. })
        | RuntimeEvent::Session(SessionEvent::ModelResponse { .. }) => {}
        RuntimeEvent::Tool(ToolEvent::Use { name, input }) => {
            if name == crate::tools::todo::TODO_WRITE_TOOL_NAME {
                if let Ok(state) = crate::todo::normalize_todo_write_input(&input) {
                    app.snapshot.todo = crate::context::TodoContextView::from_state(Some(state));
                } else {
                    log::warn!("failed to normalize todo_write runtime event");
                }
                return;
            }
            if let Some(event) = TerminalEvent::from_tool_use(&name, &input) {
                apply_tui_event(app, TuiEvent::Terminal(event));
                return;
            }
            app.finalize_agent_stream(None);
            if let Some(action) = exploration_action_label_for(&name, &input) {
                app.record_exploration_action(action);
            } else if let Some(action) = planning_action_label_for(&name, &input) {
                app.record_planning_action(action);
            } else if let Some(action) = tool_action_label_for(&name, &input) {
                app.record_running_action(action);
            }
            app.set_runtime_phase(RuntimePhase::RunningTool, Some(name.clone()));
            app.push_entry("Tool", format_tool_use(&name, &input));
        }
        RuntimeEvent::Tool(ToolEvent::Result {
            name,
            content,
            is_error,
        }) => {
            if name == crate::tools::todo::TODO_WRITE_TOOL_NAME || is_exploration_tool_name(&name) {
                return;
            }
            if let Some(event) = TerminalEvent::from_tool_result(&name, &content, is_error) {
                apply_tui_event(app, TuiEvent::Terminal(event));
                return;
            }
            app.finalize_agent_stream(None);
            if let Some(request) = subagent_request_input(&content) {
                app.advance_running_tool_boundary();
                let source = name.as_str();
                app.record_local_request_input(
                    source,
                    request.question,
                    request.options,
                    request.note,
                );
            } else if let Some(note) = exploration_result_note(&content) {
                app.advance_running_tool_boundary();
                app.record_exploration_note(note);
            } else if let Some(note) = planning_result_note(&content) {
                app.advance_running_tool_boundary();
                app.record_planning_note(note);
            } else {
                app.advance_running_tool_boundary();
            }
            app.set_runtime_phase(RuntimePhase::RunningTool, Some(name.clone()));
            app.push_entry(
                if is_error {
                    "Tool Error"
                } else {
                    "Tool Result"
                },
                format_tool_result(&name, &content),
            );
        }
        RuntimeEvent::Tool(ToolEvent::Progress {
            name,
            stream,
            chunk,
        }) => {
            if let Some(event) = TerminalEvent::from_tool_progress(&name, stream.into(), &chunk) {
                apply_tui_event(app, TuiEvent::Terminal(event));
            } else {
                apply_tui_event(
                    app,
                    TuiEvent::ToolProgress {
                        name,
                        stream: stream.into(),
                        chunk,
                    },
                );
            }
        }
        RuntimeEvent::Memory(event) => {
            app.push_system(
                format_memory_event_notice(&event),
                SystemMessageKind::Memory,
            );
        }
        RuntimeEvent::Todo(crate::runtime_control::TodoEvent::Updated { state }) => {
            app.snapshot.todo = crate::context::TodoContextView::from_state(Some(state));
        }
        RuntimeEvent::Plan(crate::runtime_control::PlanEvent::Updated { steps, explanation }) => {
            app.snapshot.plan_steps = steps
                .into_iter()
                .map(|step| {
                    let status = match step.status {
                        crate::runtime_control::PlanStepStatusEvent::Pending => "pending",
                        crate::runtime_control::PlanStepStatusEvent::InProgress => "in_progress",
                        crate::runtime_control::PlanStepStatusEvent::Completed => "completed",
                    };
                    (step.step, status.to_string())
                })
                .collect();
            app.snapshot.plan_explanation = explanation;
        }
        RuntimeEvent::Plan(_) | RuntimeEvent::Approval(_) | RuntimeEvent::Mcp(_) => {}
        RuntimeEvent::Warning(crate::runtime_control::WarningEvent::RuntimeWarning { message }) => {
            app.push_system(message, SystemMessageKind::Other);
        }
        RuntimeEvent::Error(crate::runtime_control::ErrorEvent::RuntimeError {
            message, ..
        }) => {
            app.push_system(message, SystemMessageKind::Other);
        }
        RuntimeEvent::Input(_)
        | RuntimeEvent::PromptSource(_)
        | RuntimeEvent::Skill(_)
        | RuntimeEvent::Hook(_)
        | RuntimeEvent::Context(_)
        | RuntimeEvent::Extension(_) => {}
    }
}

fn apply_assistant_text(app: &mut TuiApp, message: String) {
    let message = scrub_internal_control_tokens(&message);
    if message.trim().is_empty() {
        app.set_runtime_phase(
            RuntimePhase::ProcessingResponse,
            Some("receiving model output".into()),
        );
        return;
    }

    let planning_mode = matches!(
        app.agent_execution_mode,
        crate::agent::AgentExecutionMode::Plan
    );
    let structured_planning_output = contains_structured_planning_output(&message);
    let has_live_exploration = !app.active_live.exploration_actions.is_empty()
        || !app.active_live.exploration_notes.is_empty();
    let planning_notes = if planning_mode && !structured_planning_output {
        planning_note_lines(&message)
    } else {
        Vec::new()
    };
    if !app.active_live.exploration_actions.is_empty()
        && matches!(
            app.runtime_phase,
            RuntimePhase::RunningTool | RuntimePhase::SendingPrompt
        )
        && (!planning_mode || (planning_notes.is_empty() && !structured_planning_output))
    {
        for note in exploration_note_lines(&message, planning_mode) {
            app.record_exploration_note(note);
        }
    }
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("receiving model output".into()),
    );
    if planning_mode && !structured_planning_output {
        for note in planning_notes {
            app.record_planning_note(note);
        }
        if has_live_exploration
            || !app.active_live.planning_actions.is_empty()
            || !app.active_live.planning_notes.is_empty()
        {
            app.agent_markdown_stream = None;
            return;
        }
    }
    app.finalize_agent_stream(Some(message));
}

pub(super) fn runtime_event_from_agent_event(
    event: AgentEvent,
    provenance: crate::runtime_control::RuntimeProvenance,
) -> TuiEvent {
    TuiEvent::Runtime(Box::new(crate::runtime_control::wrap_agent_event(
        uuid::Uuid::new_v4().to_string(),
        0,
        provenance,
        event,
    )))
}

#[cfg(test)]
pub(super) fn convert_agent_event(event: AgentEvent) -> Option<TuiEvent> {
    match event {
        AgentEvent::Status(message) => Some(TuiEvent::Transcript {
            role: "Status",
            message,
        }),
        AgentEvent::AssistantText(text) => Some(TuiEvent::Transcript {
            role: "Agent",
            message: text,
        }),
        AgentEvent::AssistantDelta(text) => Some(TuiEvent::Transcript {
            role: "Agent Delta",
            message: text,
        }),
        AgentEvent::AssistantThinkingDelta(text) => Some(TuiEvent::Transcript {
            role: "Agent Thinking Delta",
            message: text,
        }),
        AgentEvent::ToolUse { name, input } => {
            if name == crate::tools::todo::TODO_WRITE_TOOL_NAME {
                match crate::todo::normalize_todo_write_input(&input) {
                    Ok(state) => {
                        return Some(TuiEvent::UpdateTodo(
                            crate::context::TodoContextView::from_state(Some(state)),
                        ));
                    }
                    Err(e) => {
                        eprintln!("todo_write parse error: {e}");
                    }
                }
                return None;
            }
            if let Some(event) = TerminalEvent::from_tool_use(&name, &input) {
                return Some(TuiEvent::Terminal(event));
            }
            Some(TuiEvent::Transcript {
                role: "Tool",
                message: format_tool_use(&name, &input),
            })
        }
        AgentEvent::ToolResult {
            name,
            content,
            is_error,
        } => {
            if name == crate::tools::todo::TODO_WRITE_TOOL_NAME {
                return None;
            }
            if is_exploration_tool_name(&name) {
                return None;
            }
            if let Some(event) = TerminalEvent::from_tool_result(&name, &content, is_error) {
                return Some(TuiEvent::Terminal(event));
            }
            Some(TuiEvent::Transcript {
                role: if is_error {
                    "Tool Error"
                } else {
                    "Tool Result"
                },
                message: format_tool_result(&name, &content),
            })
        }
        AgentEvent::ToolProgress {
            name,
            stream,
            chunk,
        } => TerminalEvent::from_tool_progress(&name, stream, &chunk)
            .map(TuiEvent::Terminal)
            .or({
                Some(TuiEvent::ToolProgress {
                    name,
                    stream,
                    chunk,
                })
            }),
        AgentEvent::MemoryAction { message } => Some(TuiEvent::Transcript {
            role: "System",
            message,
        }),
        AgentEvent::TodoUpdated(state) => Some(TuiEvent::Transcript {
            role: "Todo",
            message: format_todo_update(&state),
        }),
        AgentEvent::PlanUpdated { .. }
        | AgentEvent::ApprovalRequested { .. }
        | AgentEvent::ApprovalAnswered { .. } => None,
        AgentEvent::McpStatusUpdated(_) => None,
        AgentEvent::McpStatusLoadFailed { .. } => None,
        AgentEvent::AgentStart => None,
        AgentEvent::AgentStop { .. } => None,
        AgentEvent::AgentError { .. } => None,
        AgentEvent::ModelRequest { .. } => None,
        AgentEvent::ModelResponse { .. } => None,
    }
}

pub(super) fn format_memory_event_notice(event: &MemoryEvent) -> String {
    match event {
        MemoryEvent::RecordAdded { memory_id } => {
            memory_notice(format!("wrote record {}", short_memory_id(memory_id)))
        }
        MemoryEvent::RecordUpdated { memory_id } => {
            memory_notice(format!("updated record {}", short_memory_id(memory_id)))
        }
        MemoryEvent::RecordDeleted { memory_id } => {
            memory_notice(format!("deleted record {}", short_memory_id(memory_id)))
        }
        MemoryEvent::LabelsListed { labels, .. } => memory_notice(format!(
            "listed labels: {} {}",
            labels.len(),
            count_label("label", labels.len())
        )),
        MemoryEvent::MetadataQueried { record_count, .. } => memory_notice(format!(
            "queried metadata: {} {}",
            record_count,
            count_label("record", *record_count)
        )),
        MemoryEvent::RecordsQueried { query, records } => memory_notice(format!(
            "queried records for \"{}\": {} {}",
            memory_query_preview(query),
            records.len(),
            count_label("result", records.len())
        )),
        MemoryEvent::ActionObserved { message } => message.clone(),
        MemoryEvent::SessionShardPromotionObserved { outcome } => {
            memory_notice(format_session_shard_promotion_outcome(outcome))
        }
        MemoryEvent::SelectionUpdated => memory_notice("refreshed selection snapshot"),
    }
}

fn memory_query_preview(query: &str) -> String {
    let sanitized = sanitize_display_text(&redact_secrets(query));
    let condensed = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_memory_query_preview(&condensed)
}

fn truncate_memory_query_preview(query: &str) -> String {
    if query.chars().count() <= MEMORY_QUERY_PREVIEW_LIMIT {
        return query.to_string();
    }

    let mut truncated = query
        .chars()
        .take(MEMORY_QUERY_PREVIEW_LIMIT.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn format_session_shard_promotion_outcome(outcome: &SessionShardPromotionOutcome) -> String {
    let checkpoint_count = outcome.plan.checkpoint_count;
    match &outcome.plan.decision {
        SessionShardPromotionDecision::Eligible => format!(
            "promoted session shards: {} {} from {} {}",
            outcome.promoted_count,
            count_label("record", outcome.promoted_count),
            checkpoint_count,
            count_label("checkpoint", checkpoint_count)
        ),
        SessionShardPromotionDecision::Skipped { reason } => format!(
            "skipped session shard promotion: {} with {} {}",
            session_shard_skip_reason_label(reason),
            checkpoint_count,
            count_label("checkpoint", checkpoint_count)
        ),
    }
}

fn session_shard_skip_reason_label(reason: &SessionShardPromotionSkipReason) -> &'static str {
    match reason {
        SessionShardPromotionSkipReason::Disabled => "disabled",
        SessionShardPromotionSkipReason::Empty => "no checkpoints",
        SessionShardPromotionSkipReason::BelowMinCheckpoints => "below minimum checkpoints",
        SessionShardPromotionSkipReason::MaxCheckpointsZero => "max checkpoints is zero",
    }
}

fn short_memory_id(memory_id: &str) -> &str {
    memory_id
        .char_indices()
        .nth(12)
        .map_or(memory_id, |(idx, _)| &memory_id[..idx])
}

pub(super) fn format_error_chain(err: &anyhow::Error) -> String {
    helpers::format_error_chain(err)
}
