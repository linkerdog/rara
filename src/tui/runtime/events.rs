mod helpers;
#[cfg(test)]
mod tests;

use self::helpers::{
    append_tool_progress, exploration_action_label, exploration_note_lines,
    exploration_result_note, format_tool_result, format_tool_use, is_exploration_tool_name,
    is_oauth_prompt_message, planning_action_label, planning_note_lines, planning_result_note,
    scrub_internal_control_tokens, subagent_request_input, tool_action_label,
};
use super::super::state::{RuntimePhase, TuiApp, TuiEvent, contains_structured_planning_output};
use crate::agent::AgentEvent;
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
                    app.push_entry("System", message);
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
            app.push_entry(role, message)
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
            if role == "Tool" {
                if let Some(action) = tool_action_label(&message) {
                    app.record_running_action(action);
                }
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
            .or_else(|| {
                Some(TuiEvent::ToolProgress {
                    name,
                    stream,
                    chunk,
                })
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

pub(super) fn format_error_chain(err: &anyhow::Error) -> String {
    helpers::format_error_chain(err)
}
