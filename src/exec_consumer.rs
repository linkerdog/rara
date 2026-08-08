use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rara_tools::tool::ToolOutputStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::agent::{Agent, AgentEvent, AgentOutputMode};

#[derive(Debug, Clone)]
pub struct ExecRunOptions {
    pub prompt: Option<String>,
    pub json: bool,
    pub output_last_message: Option<PathBuf>,
    pub run_id: Option<String>,
    pub task_id: Option<String>,
}

pub struct ExecConsumer {
    agent: Agent,
    options: ExecRunOptions,
}

impl ExecConsumer {
    pub fn new(agent: Agent, options: ExecRunOptions) -> Self {
        Self { agent, options }
    }

    pub async fn run(mut self) -> Result<()> {
        let prompt = read_exec_prompt(self.options.prompt.clone())?;
        let mut processor = ExecJsonlProcessor::new(
            ExecRunMetadata {
                session_id: self.agent.session_id.clone(),
                run_id: self.options.run_id.clone(),
                task_id: self.options.task_id.clone(),
            },
            self.options.json,
        );

        processor.emit_thread_started()?;
        processor.emit_turn_started()?;

        let result = self
            .agent
            .query_with_mode_and_events(prompt, AgentOutputMode::Silent, |event| {
                if let Err(err) = processor.process_event(event) {
                    eprintln!("failed to process exec event: {err}");
                }
            })
            .await;

        match result {
            Ok(()) => {
                processor.update_usage(
                    self.agent.total_input_tokens,
                    self.agent.total_output_tokens,
                );
                if processor.final_message().is_none() {
                    let message =
                        "headless exec completed without a final assistant message".to_string();
                    processor.emit_turn_failed(message.clone())?;
                    bail!("{message}");
                }
                processor.emit_turn_completed()?;
                if !self.options.json
                    && let Some(message) = processor.final_message()
                {
                    println!("{message}");
                }
                if let Some(path) = self.options.output_last_message.as_deref() {
                    write_last_message(path, processor.final_message())?;
                }
                Ok(())
            }
            Err(err) => {
                processor.update_usage(
                    self.agent.total_input_tokens,
                    self.agent.total_output_tokens,
                );
                processor.emit_turn_failed(err.to_string())?;
                Err(err)
            }
        }
    }
}

pub fn emit_exec_startup_failure_jsonl(
    run_id: Option<String>,
    task_id: Option<String>,
    message: String,
) {
    for event in exec_startup_failure_events(run_id, task_id, message) {
        let _ = emit_jsonl(&event);
    }
}

pub fn exec_startup_failure_events(
    run_id: Option<String>,
    task_id: Option<String>,
    message: String,
) -> Vec<ExecEvent> {
    vec![
        ExecEvent::ThreadStarted {
            metadata: ExecRunMetadata {
                session_id: "startup".to_string(),
                run_id,
                task_id,
            },
            timestamp: timestamp_now(),
        },
        ExecEvent::TurnStarted {
            timestamp: timestamp_now(),
        },
        ExecEvent::TurnFailed {
            error: ExecError { message },
            timestamp: timestamp_now(),
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecRunMetadata {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecEvent {
    #[serde(rename = "thread.started")]
    ThreadStarted {
        metadata: ExecRunMetadata,
        timestamp: String,
    },
    #[serde(rename = "turn.started")]
    TurnStarted { timestamp: String },
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        usage: ExecUsage,
        final_message: Option<String>,
        timestamp: String,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: ExecError, timestamp: String },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: ExecItem },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecError {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecItem {
    pub id: String,
    #[serde(flatten)]
    pub details: ExecItemDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecItemDetails {
    Status {
        message: String,
    },
    AgentMessage {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolCall {
        name: String,
        input: Value,
    },
    ToolResult {
        name: String,
        content: String,
        is_error: bool,
    },
    ToolProgress {
        name: String,
        stream: ExecToolStream,
        chunk: String,
    },
    MemoryAction {
        message: String,
    },
    TodoUpdated,
    ModelRequest {
        model: String,
        input_tokens: u32,
    },
    ModelResponse {
        model: String,
        output_tokens: u32,
        finish_reason: Option<String>,
    },
    Error {
        message: String,
        recoverable: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecToolStream {
    Stdout,
    Stderr,
}

impl From<ToolOutputStream> for ExecToolStream {
    fn from(value: ToolOutputStream) -> Self {
        match value {
            ToolOutputStream::Stdout => Self::Stdout,
            ToolOutputStream::Stderr => Self::Stderr,
        }
    }
}

struct ExecJsonlProcessor {
    metadata: ExecRunMetadata,
    emit_json: bool,
    next_item_id: u64,
    final_message: Option<String>,
    usage: ExecUsage,
}

impl ExecJsonlProcessor {
    fn new(metadata: ExecRunMetadata, emit_json: bool) -> Self {
        Self {
            metadata,
            emit_json,
            next_item_id: 0,
            final_message: None,
            usage: ExecUsage::default(),
        }
    }

    fn final_message(&self) -> Option<&str> {
        self.final_message.as_deref()
    }

    fn update_usage(&mut self, input_tokens: u32, output_tokens: u32) {
        self.usage.input_tokens = input_tokens;
        self.usage.output_tokens = output_tokens;
    }

    fn next_item_id(&mut self) -> String {
        let id = format!("item_{}", self.next_item_id);
        self.next_item_id += 1;
        id
    }

    fn emit_thread_started(&self) -> Result<()> {
        self.emit(ExecEvent::ThreadStarted {
            metadata: self.metadata.clone(),
            timestamp: timestamp_now(),
        })
    }

    fn emit_turn_started(&self) -> Result<()> {
        self.emit(ExecEvent::TurnStarted {
            timestamp: timestamp_now(),
        })
    }

    fn emit_turn_completed(&self) -> Result<()> {
        self.emit(ExecEvent::TurnCompleted {
            usage: self.usage.clone(),
            final_message: self.final_message.clone(),
            timestamp: timestamp_now(),
        })
    }

    fn emit_turn_failed(&self, message: String) -> Result<()> {
        self.emit(ExecEvent::TurnFailed {
            error: ExecError { message },
            timestamp: timestamp_now(),
        })
    }

    fn process_event(&mut self, event: AgentEvent) -> Result<()> {
        match event {
            AgentEvent::AgentStart => Ok(()),
            AgentEvent::AgentStop { .. } => Ok(()),
            AgentEvent::PlanUpdated { .. }
            | AgentEvent::ApprovalRequested { .. }
            | AgentEvent::ApprovalAnswered { .. }
            | AgentEvent::Compaction { .. } => Ok(()),
            AgentEvent::AssistantText(text) => {
                self.final_message
                    .get_or_insert_with(String::new)
                    .push_str(&text);
                self.emit_item(ExecItemDetails::AgentMessage { text })
            }
            AgentEvent::AssistantDelta(delta) => {
                self.final_message
                    .get_or_insert_with(String::new)
                    .push_str(delta.as_str());
                self.emit_item(ExecItemDetails::AgentMessage { text: delta })
            }
            AgentEvent::AssistantThinkingDelta(text) => {
                self.emit_item(ExecItemDetails::Reasoning { text })
            }
            AgentEvent::Status(message) => self.emit_item(ExecItemDetails::Status { message }),
            AgentEvent::ToolUse { name, input } => {
                self.emit_item(ExecItemDetails::ToolCall { name, input })
            }
            AgentEvent::ToolResult {
                name,
                content,
                is_error,
            } => self.emit_item(ExecItemDetails::ToolResult {
                name,
                content,
                is_error,
            }),
            AgentEvent::ToolProgress {
                name,
                stream,
                chunk,
            } => self.emit_item(ExecItemDetails::ToolProgress {
                name,
                stream: stream.into(),
                chunk,
            }),
            AgentEvent::MemoryAction { message } => {
                self.emit_item(ExecItemDetails::MemoryAction { message })
            }
            AgentEvent::TodoUpdated(_) => self.emit_item(ExecItemDetails::TodoUpdated),
            AgentEvent::ModelRequest {
                model,
                input_tokens,
            } => {
                self.usage.input_tokens = input_tokens;
                self.emit_item(ExecItemDetails::ModelRequest {
                    model,
                    input_tokens,
                })
            }
            AgentEvent::ModelResponse {
                model,
                output_tokens,
                finish_reason,
            } => {
                self.usage.output_tokens = self.usage.output_tokens.saturating_add(output_tokens);
                self.emit_item(ExecItemDetails::ModelResponse {
                    model,
                    output_tokens,
                    finish_reason,
                })
            }
            AgentEvent::AgentError {
                message,
                recoverable,
            } => self.emit_item(ExecItemDetails::Error {
                message,
                recoverable,
            }),
            AgentEvent::McpStatusUpdated(_) | AgentEvent::McpStatusLoadFailed { .. } => Ok(()),
        }
    }

    fn emit_item(&mut self, details: ExecItemDetails) -> Result<()> {
        let item = ExecItem {
            id: self.next_item_id(),
            details,
        };
        self.emit(ExecEvent::ItemCompleted { item })
    }

    fn emit(&self, event: ExecEvent) -> Result<()> {
        if self.emit_json {
            emit_jsonl(&event)?;
        }
        Ok(())
    }
}

fn emit_jsonl(event: &ExecEvent) -> Result<()> {
    println!("{}", serde_json::to_string(event)?);
    Ok(())
}

fn timestamp_now() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn write_last_message(path: &std::path::Path, message: Option<&str>) -> Result<()> {
    let message = message.unwrap_or_default();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, message).with_context(|| format!("failed to write {}", path.display()))
}

pub fn read_exec_prompt(prompt: Option<String>) -> Result<String> {
    let stdin_is_terminal = io::stdin().is_terminal();
    let stdin = if stdin_is_terminal {
        None
    } else {
        Some(read_stdin_to_string()?)
    };
    resolve_exec_prompt(prompt, stdin)
}

fn read_stdin_to_string() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("failed to read prompt from stdin")?;
    Ok(input)
}

fn resolve_exec_prompt(prompt: Option<String>, stdin: Option<String>) -> Result<String> {
    match (prompt, stdin) {
        (Some(prompt), Some(stdin)) if prompt == "-" => require_non_empty_stdin(stdin),
        (Some(prompt), Some(stdin)) if stdin.trim().is_empty() => Ok(prompt),
        (Some(prompt), Some(stdin)) => Ok(format!("{prompt}\n\n<stdin>\n{stdin}</stdin>")),
        (Some(prompt), None) if prompt == "-" => bail!("No prompt provided via stdin."),
        (Some(prompt), None) => Ok(prompt),
        (None, Some(stdin)) => require_non_empty_stdin(stdin),
        (None, None) => bail!("No prompt provided via stdin."),
    }
}

fn require_non_empty_stdin(stdin: String) -> Result<String> {
    if stdin.trim().is_empty() {
        bail!("No prompt provided via stdin.");
    }
    Ok(stdin)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn prompt_argument_appends_non_empty_stdin_block() {
        let prompt = resolve_exec_prompt(Some("Summarize this".to_string()), Some("data\n".into()))
            .expect("prompt");

        assert_eq!(prompt, "Summarize this\n\n<stdin>\ndata\n</stdin>");
    }

    #[test]
    fn dash_prompt_reads_stdin_as_prompt() {
        let prompt = resolve_exec_prompt(Some("-".to_string()), Some("from stdin\n".into()))
            .expect("prompt");

        assert_eq!(prompt, "from stdin\n");
    }

    #[test]
    fn missing_prompt_rejects_empty_stdin() {
        let err = resolve_exec_prompt(None, Some(String::new())).expect_err("empty stdin");

        assert_eq!(err.to_string(), "No prompt provided via stdin.");
    }

    #[test]
    fn processor_maps_agent_events_to_exec_items() {
        let mut processor = ExecJsonlProcessor::new(
            ExecRunMetadata {
                session_id: "session-1".to_string(),
                run_id: Some("run-1".to_string()),
                task_id: Some("task-1".to_string()),
            },
            false,
        );

        processor
            .process_event(AgentEvent::AssistantText("done".to_string()))
            .expect("assistant event");
        processor
            .process_event(AgentEvent::ToolUse {
                name: "bash".to_string(),
                input: json!({"cmd": "true"}),
            })
            .expect("tool event");

        assert_eq!(processor.final_message(), Some("done"));
        assert_eq!(processor.next_item_id, 2);
    }

    #[test]
    fn processor_appends_assistant_text_parts() {
        let mut processor = ExecJsonlProcessor::new(test_metadata(), false);

        processor
            .process_event(AgentEvent::AssistantText("first ".to_string()))
            .expect("first assistant text");
        processor
            .process_event(AgentEvent::AssistantText("second ".to_string()))
            .expect("second assistant text");
        processor
            .process_event(AgentEvent::AssistantDelta("third".to_string()))
            .expect("assistant delta");

        assert_eq!(processor.final_message(), Some("first second third"));
    }

    #[test]
    fn processor_updates_usage_from_final_agent_totals() {
        let mut processor = ExecJsonlProcessor::new(test_metadata(), false);

        processor
            .process_event(AgentEvent::ModelRequest {
                model: "mock".to_string(),
                input_tokens: 0,
            })
            .expect("model request");
        processor
            .process_event(AgentEvent::ModelResponse {
                model: "mock".to_string(),
                output_tokens: 2,
                finish_reason: Some("stop".to_string()),
            })
            .expect("model response");
        processor.update_usage(13, 7);

        assert_eq!(processor.usage.input_tokens, 13);
        assert_eq!(processor.usage.output_tokens, 7);
    }

    #[test]
    fn startup_failure_events_preserve_harness_metadata() {
        let events = exec_startup_failure_events(
            Some("run-1".to_string()),
            Some("task-1".to_string()),
            "rara exec panicked during startup".to_string(),
        );

        assert_eq!(events.len(), 3);
        match &events[0] {
            ExecEvent::ThreadStarted { metadata, .. } => {
                assert_eq!(metadata.session_id, "startup");
                assert_eq!(metadata.run_id.as_deref(), Some("run-1"));
                assert_eq!(metadata.task_id.as_deref(), Some("task-1"));
            }
            other => panic!("unexpected first event: {other:?}"),
        }
        assert!(matches!(events[1], ExecEvent::TurnStarted { .. }));
        match &events[2] {
            ExecEvent::TurnFailed { error, .. } => {
                assert_eq!(error.message, "rara exec panicked during startup");
            }
            other => panic!("unexpected third event: {other:?}"),
        }
    }

    fn test_metadata() -> ExecRunMetadata {
        ExecRunMetadata {
            session_id: "session-1".to_string(),
            run_id: Some("run-1".to_string()),
            task_id: Some("task-1".to_string()),
        }
    }
}
