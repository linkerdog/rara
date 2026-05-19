mod helpers;
#[cfg(test)]
mod tests;

use self::helpers::{
    append_tool_progress, exploration_action_label, exploration_note_lines,
    exploration_result_note, format_tool_result, format_tool_use, is_exploration_tool_name,
    is_oauth_prompt_message, planning_action_label, planning_note_lines, planning_result_note,
    scrub_internal_control_tokens, subagent_request_input, tool_action_label,
};
use super::super::state::{
    RuntimePhase, SystemMessageKind, TuiApp, TuiEvent, contains_structured_planning_output,
};
use crate::agent::AgentEvent;
use crate::memory_notice::{count_label, memory_notice};
use crate::runtime_control::MemoryEvent;
use crate::session_promotion::{
    SessionShardPromotionDecision, SessionShardPromotionOutcome, SessionShardPromotionSkipReason,
};
use crate::todo::format_todo_update;
use crate::tui::terminal_event::{TerminalEvent, TerminalTarget};

const TOOL_PROGRESS_LINE_LIMIT: usize = 16;

pub(super) fn apply_tui_event(app: &mut TuiApp, event: TuiEvent) {
    match event {
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
                    && (!planning_mode
                        || (planning_notes.is_empty() && !structured_planning_output))
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
        TuiEvent::UpdateTodo(view) => {
            app.snapshot.todo = view;
        }
    }
}

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
        MemoryEvent::RecordsQueried { records } => memory_notice(format!(
            "queried records: {} {}",
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
