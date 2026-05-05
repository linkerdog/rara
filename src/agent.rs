mod compact;
mod context_view;
mod memory_retrieval;
mod planning;
mod prompting;
#[cfg(test)]
mod tests;

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::context::RetrievedMemoryCandidate;
use crate::control_tokens::scrub_internal_control_tokens;
use crate::llm::{ContentBlock, LlmBackend, LlmStreamEvent, LlmTurnMetadata};
use crate::mcp_status::McpStatusSnapshot;
use crate::memory_store::MemoryStore;
use crate::prompt::{self, PromptMode, PromptRuntimeConfig};
use crate::redaction::redact_secrets;
use crate::session::SessionManager;
use crate::todo::TodoState;
use crate::tool::ToolOutputStream;
use crate::tool::{ToolCallContext, ToolManager, ToolProgressEvent};
use crate::tool_result::{
    ToolResultStore, default_tool_result_store_dir, enforce_tool_result_batch_budget,
    repair_tool_result_history,
};
use crate::tools::bash::BashCommandInput;
use crate::tools::planning::{ENTER_PLAN_MODE_TOOL_NAME, EXIT_PLAN_MODE_TOOL_NAME};
use crate::tools::todo::TODO_WRITE_TOOL_NAME;
use crate::vectordb::{MemoryMetadata, VectorDB};
use crate::workspace::WorkspaceMemory;

const MAX_RUNTIME_ERROR_RECOVERY_ATTEMPTS: usize = 1;
const MAX_PLAN_EXIT_REPAIR_ATTEMPTS: usize = 1;

pub use self::compact::{CompactBoundaryMetadata, CompactState, latest_compact_boundary_metadata};
pub use self::planning::{
    CompletedInteraction, PendingApproval, PendingUserInput, PlanStep, PlanStepStatus,
};
use self::planning::{InspectionProgress, RuntimeContinuationPhase, tool_result_message};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentExecutionMode {
    Execute,
    Plan,
    Review,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BashApprovalMode {
    Once,
    Always,
    Suggestion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BashApprovalDecision {
    Once,
    Prefix,
    Always,
    Suggestion,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentOutputMode {
    Terminal,
    Silent,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Status(String),
    AssistantText(String),
    AssistantDelta(String),
    AssistantThinkingDelta(String),
    ToolUse {
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
        stream: ToolOutputStream,
        chunk: String,
    },
    McpStatusUpdated(McpStatusSnapshot),
    McpStatusLoadFailed {
        message: String,
    },
    TodoUpdated(TodoState),
}

#[derive(Debug)]
struct ToolCall {
    id: String,
    name: String,
    input: Value,
}

#[derive(Debug)]
struct TurnOutput {
    assistant_message: Option<Message>,
    tool_calls: Vec<ToolCall>,
    plan_updated: bool,
    malformed_proposed_plan: bool,
    continue_inspection: bool,
    had_text_response: bool,
    had_reasoning_response: bool,
}

pub struct Agent {
    pub tool_manager: ToolManager,
    pub llm_backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub memory_store: Arc<MemoryStore>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub history: Vec<Message>,
    pub session_id: String,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub total_cache_hit_tokens: u32,
    pub total_cache_miss_tokens: u32,
    pub tool_result_store: ToolResultStore,
    pub execution_mode: AgentExecutionMode,
    pub bash_approval_mode: BashApprovalMode,
    pub current_plan: Vec<PlanStep>,
    pub plan_explanation: Option<String>,
    pub pending_user_input: Option<PendingUserInput>,
    pub pending_approval: Option<PendingApproval>,
    pub todo_state: Option<TodoState>,
    pub completed_user_input: Option<CompletedInteraction>,
    pub completed_approval: Option<CompletedInteraction>,
    pub approved_bash_prefixes: Vec<String>,
    pub compact_state: CompactState,
    pub retrieved_memory_candidates: Vec<RetrievedMemoryCandidate>,
    inspection_progress: InspectionProgress,
    last_query_plan_updated: bool,
    pending_plan_exit_tool_id: Option<String>,
    prompt_config: PromptRuntimeConfig,
    cancellation_token: Option<Arc<AtomicBool>>,
}

impl Agent {
    pub fn new(
        tool_manager: ToolManager,
        llm_backend: Arc<dyn LlmBackend>,
        vdb: Arc<VectorDB>,
        session_manager: Arc<SessionManager>,
        workspace: Arc<WorkspaceMemory>,
    ) -> Self {
        let memory_store = Arc::new(MemoryStore::new(llm_backend.clone(), vdb.clone()));
        Self {
            tool_manager,
            llm_backend,
            vdb,
            memory_store,
            session_manager,
            workspace,
            history: Vec::new(),
            session_id: Uuid::new_v4().to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_hit_tokens: 0,
            total_cache_miss_tokens: 0,
            tool_result_store: ToolResultStore::new(
                default_tool_result_store_dir().expect("tool result store dir"),
            )
            .expect("tool result store"),
            execution_mode: AgentExecutionMode::Execute,
            bash_approval_mode: BashApprovalMode::Always,
            current_plan: Vec::new(),
            plan_explanation: None,
            pending_user_input: None,
            pending_approval: None,
            todo_state: None,
            completed_user_input: None,
            completed_approval: None,
            approved_bash_prefixes: Vec::new(),
            compact_state: CompactState::default(),
            retrieved_memory_candidates: Vec::new(),
            inspection_progress: InspectionProgress::default(),
            last_query_plan_updated: false,
            pending_plan_exit_tool_id: None,
            prompt_config: PromptRuntimeConfig::default(),
            cancellation_token: None,
        }
    }

    pub async fn query(&mut self, prompt: String) -> Result<()> {
        self.query_with_mode(prompt, AgentOutputMode::Terminal)
            .await
    }

    pub async fn query_with_mode(
        &mut self,
        prompt: String,
        output_mode: AgentOutputMode,
    ) -> Result<()> {
        self.query_with_mode_and_events(prompt, output_mode, |_| {})
            .await
    }

    pub async fn query_with_mode_and_events<F>(
        &mut self,
        prompt: String,
        output_mode: AgentOutputMode,
        mut report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let turn_start_idx = self.history.len();
        let mut agentic_turns = 0usize;
        let mut runtime_error_recoveries = 0usize;
        self.inspection_progress = InspectionProgress::default();
        self.last_query_plan_updated = false;
        self.pending_plan_exit_tool_id = None;
        self.compact_if_needed_with_reporter(&mut report).await?;
        let repaired_history = repair_tool_result_history(&self.history);
        if repaired_history != self.history {
            self.replace_history(repaired_history);
            self.checkpoint_session()?;
        }
        self.clear_completed_interactions();

        self.push_history_message(Message {
            role: "user".to_string(),
            content: json!([{"type": "text", "text": prompt.clone()}]),
        });
        self.checkpoint_session()?;
        self.refresh_memory_retrieval_candidates().await;

        match self
            .run_agent_loop_with_limit(output_mode, &mut report, &mut agentic_turns)
            .await
        {
            Ok(()) => {}
            Err(err) => {
                if self
                    .try_continue_after_recoverable_runtime_error(
                        &err,
                        output_mode,
                        &mut report,
                        &mut agentic_turns,
                        &mut runtime_error_recoveries,
                    )
                    .await?
                {
                    report(AgentEvent::Status(
                        "Runtime error was surfaced to the model and the turn continued."
                            .to_string(),
                    ));
                } else {
                    return Err(err);
                }
            }
        }

        self.checkpoint_session()?;
        let turn_text = format!(
            "User: {}\nAgent Response: {:?}",
            prompt,
            self.history.last().unwrap().content
        );
        if let Ok(vector) = self.llm_backend.embed(&turn_text).await {
            let _ = self
                .vdb
                .upsert_turn(
                    "conversations",
                    MemoryMetadata {
                        id: None,
                        session_id: self.session_id.clone(),
                        turn_index: turn_start_idx as u32,
                        text: turn_text,
                    },
                    vector,
                )
                .await;
        }
        Ok(())
    }

    pub(super) fn checkpoint_session(&self) -> Result<()> {
        self.session_manager
            .save_session(&self.session_id, &self.history)
    }

    async fn run_model_turn<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
    ) -> Result<TurnOutput>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let tool_schemas = self.visible_tool_schemas();
        self.run_model_turn_with_tools(output_mode, report, tool_schemas.as_slice())
            .await
    }

    async fn run_model_turn_with_tools<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
        tool_schemas: &[Value],
    ) -> Result<TurnOutput>
    where
        F: FnMut(AgentEvent) + Send,
    {
        report(AgentEvent::Status("Sending prompt to model.".to_string()));
        let turn_metadata = self.llm_turn_metadata();
        turn_metadata.ensure_not_cancelled()?;
        let assembled = self.assemble_turn_context();
        let mut messages = self
            .history
            .iter()
            .filter(|message| !is_compact_boundary_message(message))
            .cloned()
            .collect::<Vec<_>>();
        messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: json!(assembled.prompt.effective_prompt.text),
            },
        );
        if let Some(memory_context) = Agent::selected_memory_context_text(&assembled.runtime) {
            Agent::prepend_memory_context_to_latest_user_message(&mut messages, memory_context);
        }

        let mut streamed_any_text_delta = false;
        let mut streamed_any_reasoning_delta = false;
        let response = self
            .llm_backend
            .ask_streaming_with_context(&messages, tool_schemas, turn_metadata, &mut |event| {
                match event {
                    LlmStreamEvent::TextDelta(delta) => {
                        streamed_any_text_delta = true;
                        report(AgentEvent::AssistantDelta(delta));
                    }
                    LlmStreamEvent::ReasoningDelta(delta) => {
                        streamed_any_reasoning_delta |= !delta.trim().is_empty();
                        report(AgentEvent::AssistantThinkingDelta(delta));
                    }
                }
            })
            .await?;

        if let Some(usage) = &response.usage {
            self.total_input_tokens += usage.input_tokens;
            self.total_output_tokens += usage.output_tokens;
            self.total_cache_hit_tokens += usage.cache_hit_tokens;
            self.total_cache_miss_tokens += usage.cache_miss_tokens;
        }

        let mut tool_calls = Vec::new();
        let mut plan_updated = false;
        let mut malformed_proposed_plan = false;
        let mut continue_inspection = false;
        let mut had_text_response = false;
        let mut had_reasoning_response = streamed_any_reasoning_delta;
        let mut sanitized_content = Vec::new();
        for block in &response.content {
            match block {
                ContentBlock::Text { text } => {
                    let (clean_text, block_requests_continue) =
                        planning::strip_continue_inspection_control(text);
                    continue_inspection |= block_requests_continue;
                    let clean_text = scrub_internal_control_tokens(&clean_text);
                    if !clean_text.trim().is_empty() {
                        had_text_response = true;
                        sanitized_content.push(ContentBlock::Text {
                            text: clean_text.clone(),
                        });
                        if !streamed_any_text_delta {
                            report(AgentEvent::AssistantText(clean_text.clone()));
                        }
                        if matches!(self.execution_mode, AgentExecutionMode::Plan) {
                            malformed_proposed_plan |=
                                planning::has_unclosed_proposed_plan_block(&clean_text);
                            plan_updated |= self.capture_plan_from_text(&clean_text)?;
                        }
                        if matches!(output_mode, AgentOutputMode::Terminal) {
                            println!("Agent: {}", clean_text);
                        }
                    }
                }
                ContentBlock::ToolUse { id, name, input } => {
                    if matches!(self.execution_mode, AgentExecutionMode::Plan)
                        && name == EXIT_PLAN_MODE_TOOL_NAME
                        && !plan_updated
                    {
                        if let Some((steps, explanation)) =
                            planning::parse_exit_plan_tool_input(input)
                        {
                            self.current_plan = steps;
                            self.plan_explanation = explanation;
                            plan_updated = true;
                        }
                    }
                    sanitized_content.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                    report(AgentEvent::ToolUse {
                        name: name.clone(),
                        input: input.clone(),
                    });
                    tool_calls.push(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }
                ContentBlock::ProviderMetadata {
                    provider,
                    key,
                    value,
                } => {
                    sanitized_content.push(ContentBlock::ProviderMetadata {
                        provider: provider.clone(),
                        key: key.clone(),
                        value: value.clone(),
                    });
                    if key == "reasoning_content"
                        && value.as_str().is_some_and(|text| !text.trim().is_empty())
                    {
                        had_reasoning_response = true;
                    }
                }
            }
        }
        if matches!(self.execution_mode, AgentExecutionMode::Plan) && plan_updated {
            self.save_current_plan_file()?;
        }

        Ok(TurnOutput {
            assistant_message: assistant_turn_history_message(sanitized_content)?,
            tool_calls,
            plan_updated,
            malformed_proposed_plan,
            continue_inspection,
            had_text_response,
            had_reasoning_response,
        })
    }

    fn llm_turn_metadata(&self) -> LlmTurnMetadata {
        let metadata = match self.execution_mode {
            AgentExecutionMode::Execute | AgentExecutionMode::Review => LlmTurnMetadata::execute(),
            AgentExecutionMode::Plan => LlmTurnMetadata::plan(),
        };
        if let Some(token) = self.cancellation_token.as_ref() {
            metadata.with_cancellation(token.clone())
        } else {
            metadata
        }
    }

    async fn run_agent_loop<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut agentic_turns = 0usize;
        self.run_agent_loop_with_limit(output_mode, report, &mut agentic_turns)
            .await
    }

    async fn run_agent_loop_with_limit<F>(
        &mut self,
        output_mode: AgentOutputMode,
        report: &mut F,
        agentic_turns: &mut usize,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut plan_exit_repair_attempts = 0usize;
        loop {
            self.ensure_active_plan_step();
            let mut turn_output = self.run_model_turn(output_mode, report).await?;
            self.last_query_plan_updated = turn_output.plan_updated;
            if turn_output
                .tool_calls
                .iter()
                .any(|tool_call| tool_call.name == EXIT_PLAN_MODE_TOOL_NAME)
                && (turn_output.malformed_proposed_plan || !turn_output.plan_updated)
            {
                let content = if turn_output.malformed_proposed_plan {
                    incomplete_proposed_plan_error()
                } else {
                    missing_proposed_plan_error()
                };
                report(AgentEvent::ToolResult {
                    name: EXIT_PLAN_MODE_TOOL_NAME.to_string(),
                    content: content.clone(),
                    is_error: true,
                });
                if plan_exit_repair_attempts < MAX_PLAN_EXIT_REPAIR_ATTEMPTS {
                    plan_exit_repair_attempts += 1;
                    *agentic_turns += 1;
                    report(AgentEvent::Status(
                        "Plan exit was missing a structured proposed plan. Asking the model to repair the submission."
                            .to_string(),
                    ));
                    self.push_history_message(self.runtime_continuation_message(
                        RuntimeContinuationPhase::PlanExitRepairRequired,
                        *agentic_turns,
                    ));
                    self.checkpoint_session()?;
                    continue;
                }
                self.checkpoint_session()?;
                break;
            }
            if let Some(message) = turn_output.assistant_message.take() {
                self.push_history_message(message);
                self.checkpoint_session()?;
            }

            if turn_output.tool_calls.is_empty() {
                if self.should_continue_plan_without_tools(
                    turn_output.plan_updated,
                    turn_output.continue_inspection,
                    turn_output.had_text_response,
                    turn_output.had_reasoning_response,
                    *agentic_turns,
                ) {
                    report(AgentEvent::Status(
                        "Plan mode needs more evidence. Continuing in read-only mode.".to_string(),
                    ));
                    *agentic_turns += 1;
                    let phase = RuntimeContinuationPhase::PlanContinuationRequired;
                    self.push_history_message(
                        self.runtime_continuation_message(phase, *agentic_turns),
                    );
                    self.checkpoint_session()?;
                    continue;
                }
                if self.should_continue_execute_without_tools(
                    *agentic_turns,
                    turn_output.continue_inspection,
                    turn_output.had_text_response,
                    turn_output.had_reasoning_response,
                ) {
                    let phase = if Self::is_reasoning_only_initial_turn(
                        turn_output.had_text_response,
                        turn_output.had_reasoning_response,
                        *agentic_turns,
                    ) {
                        report(AgentEvent::Status(
                            "Model produced reasoning only. Continuing for a visible answer or tool call."
                                .to_string(),
                        ));
                        RuntimeContinuationPhase::ReasoningOnlyContinuationRequired
                    } else {
                        report(AgentEvent::Status(
                            "Repository review needs more code inspection. Continuing the same turn."
                                .to_string(),
                        ));
                        RuntimeContinuationPhase::ExecutionContinuationRequired
                    };
                    *agentic_turns += 1;
                    self.push_history_message(
                        self.runtime_continuation_message(phase, *agentic_turns),
                    );
                    self.checkpoint_session()?;
                    continue;
                }
                self.complete_active_plan_step();
                break;
            }
            *agentic_turns += 1;

            let tool_results = self
                .execute_tool_calls(turn_output.tool_calls, report)
                .await?;
            if self.pending_approval.is_some() || self.pending_plan_exit_tool_id.is_some() {
                self.checkpoint_session()?;
                break;
            }
            self.advance_plan_step();
            self.extend_history_for_next_turn(tool_results, report, *agentic_turns)?;
        }
        Ok(())
    }

    async fn try_continue_after_recoverable_runtime_error<F>(
        &mut self,
        err: &anyhow::Error,
        output_mode: AgentOutputMode,
        report: &mut F,
        agentic_turns: &mut usize,
        runtime_error_recoveries: &mut usize,
    ) -> Result<bool>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let Some(kind) = recoverable_runtime_error_kind(err) else {
            return Ok(false);
        };
        if *runtime_error_recoveries >= MAX_RUNTIME_ERROR_RECOVERY_ATTEMPTS {
            return Ok(false);
        }
        *runtime_error_recoveries += 1;
        report(AgentEvent::Status(format!(
            "Recoverable local runtime error detected ({kind}). Asking the model to handle it."
        )));
        self.push_history_message(recoverable_runtime_error_message(kind, err));
        self.run_agent_loop_with_limit(output_mode, report, agentic_turns)
            .await?;
        Ok(true)
    }

    async fn execute_tool_calls<F>(
        &mut self,
        tool_calls: Vec<ToolCall>,
        report: &mut F,
    ) -> Result<Vec<Message>>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut tool_results = Vec::new();
        let entering_plan_mode = tool_calls
            .iter()
            .any(|tool_call| tool_call.name == ENTER_PLAN_MODE_TOOL_NAME);
        if entering_plan_mode && !matches!(self.execution_mode, AgentExecutionMode::Plan) {
            self.execution_mode = AgentExecutionMode::Plan;
            report(AgentEvent::Status(
                "Entered read-only planning mode.".to_string(),
            ));
        }
        for tool_call in tool_calls {
            let tool_name = tool_call.name.clone();
            let tool_id = tool_call.id.clone();
            let tool_input = tool_call.input.clone();
            if tool_name == ENTER_PLAN_MODE_TOOL_NAME {
                let result_text = json!({
                    "status": "entered_plan_mode",
                    "instructions": [
                        "Inspect the repository with read-only tools.",
                        "Return a normal final answer for research, review, or planning-advice tasks.",
                        "Use a <proposed_plan> block only when you are requesting approval to implement a concrete plan.",
                        "Call exit_plan_mode only after the same assistant message contains a complete <proposed_plan>...</proposed_plan> block.",
                        "Use <request_user_input> only when a blocking decision needs user input.",
                        "Use <continue_inspection/> only when another read-only inspection pass is required."
                    ]
                })
                .to_string();
                report(AgentEvent::ToolResult {
                    name: tool_name,
                    content: result_text.clone(),
                    is_error: false,
                });
                tool_results.push(tool_result_message(&tool_id, result_text, false));
                continue;
            }
            if tool_name == EXIT_PLAN_MODE_TOOL_NAME {
                if self.current_plan.is_empty() {
                    let error_text = missing_proposed_plan_error();
                    report(AgentEvent::ToolResult {
                        name: tool_name.clone(),
                        content: error_text.clone(),
                        is_error: true,
                    });
                    tool_results.push(tool_result_message(&tool_id, error_text, true));
                    continue;
                }
                self.pending_plan_exit_tool_id = Some(tool_id);
                report(AgentEvent::Status(
                    "Plan ready for approval. Waiting for a structured user decision.".to_string(),
                ));
                break;
            }
            let bash_request = if tool_call.name == "bash" {
                match BashCommandInput::from_value(tool_call.input.clone()) {
                    Ok(request) => Some(request),
                    Err(err) => {
                        let error_text = format!("Error: invalid bash payload: {err}");
                        report(AgentEvent::ToolResult {
                            name: tool_name.clone(),
                            content: error_text.clone(),
                            is_error: true,
                        });
                        tool_results.push(tool_result_message(&tool_id, error_text, true));
                        continue;
                    }
                }
            } else {
                None
            };
            if let Some(request) = bash_request.as_ref()
                && matches!(self.execution_mode, AgentExecutionMode::Plan)
            {
                if !request.is_read_only() {
                    let error_text = format!(
                        "Error: bash is read-only in plan mode. Refuse command '{}' and inspect with read-only commands or return a plan.",
                        request.summary()
                    );
                    report(AgentEvent::ToolResult {
                        name: tool_name.clone(),
                        content: error_text.clone(),
                        is_error: true,
                    });
                    tool_results.push(tool_result_message(&tool_id, error_text, true));
                    continue;
                }
            }
            if let Some(request) = bash_request.as_ref()
                && (request.requires_escalated_permissions()
                    || matches!(self.bash_approval_mode, BashApprovalMode::Suggestion))
            {
                if request.is_read_only() || self.is_bash_prefix_approved(request) {
                    report(AgentEvent::Status(format!(
                        "Shell command allowed by policy: {}",
                        request.summary()
                    )));
                } else {
                    self.pending_approval = Some(PendingApproval {
                        tool_use_id: tool_id.clone(),
                        request: request.to_owned(),
                    });
                    report(AgentEvent::Status(
                        "Bash approval required. Waiting for a structured user decision."
                            .to_string(),
                    ));
                    break;
                }
            }
            if !self.is_tool_allowed_in_current_mode(&tool_name) {
                let error_text = format!(
                    "Error: tool '{}' is unavailable in {} mode. Inspect with read-only tools and return a plan instead.",
                    tool_name,
                    self.execution_mode_label()
                );
                report(AgentEvent::ToolResult {
                    name: tool_name.clone(),
                    content: error_text.clone(),
                    is_error: true,
                });
                tool_results.push(tool_result_message(&tool_id, error_text, true));
                continue;
            }
            if let Some(tool) = self.tool_manager.get_tool(&tool_name) {
                self.inspection_progress
                    .record_tool(&tool_name, &tool_input);
                let status_detail = if tool_name == "bash" {
                    BashCommandInput::from_value(tool_input.clone())
                        .map(|request| format!("Running shell command: {}", request.summary()))
                        .unwrap_or_else(|_| "Running shell command.".to_string())
                } else {
                    format!("Running tool {}.", tool_name)
                };
                report(AgentEvent::Status(status_detail));
                match tool
                    .call_with_context_events(
                        tool_input.clone(),
                        self.tool_call_context(),
                        &mut |progress| match progress {
                            ToolProgressEvent::Output { stream, chunk } => {
                                report(AgentEvent::ToolProgress {
                                    name: tool_name.clone(),
                                    stream,
                                    chunk,
                                });
                            }
                        },
                    )
                    .await
                {
                    Ok(result) => {
                        if tool_name == TODO_WRITE_TOOL_NAME {
                            let state: TodoState = serde_json::from_value(result.clone())?;
                            if let Err(err) = self
                                .session_manager
                                .save_todo_state(&self.session_id, &state)
                            {
                                report(AgentEvent::Status(format!(
                                    "Warning: failed to persist todo state: {err}"
                                )));
                            }
                            self.todo_state = Some(state.clone());
                            report(AgentEvent::TodoUpdated(state));
                        }
                        let result_text = self.tool_result_store.compact_result(
                            &tool_name,
                            &tool_id,
                            &tool_input,
                            &result,
                        )?;
                        report(AgentEvent::ToolResult {
                            name: tool_name.clone(),
                            content: result_text.clone(),
                            is_error: false,
                        });
                        tool_results.push(tool_result_message(&tool_id, result_text, false));
                    }
                    Err(e) => {
                        let error_text = format!("Error: {}", e);
                        report(AgentEvent::ToolResult {
                            name: tool_name.clone(),
                            content: error_text.clone(),
                            is_error: true,
                        });
                        tool_results.push(tool_result_message(&tool_id, error_text, true));
                    }
                }
            }
        }
        Ok(enforce_tool_result_batch_budget(tool_results))
    }

    fn tool_call_context(&self) -> ToolCallContext {
        match self.cancellation_token.as_ref() {
            Some(token) => ToolCallContext::default().with_cancellation(token.clone()),
            None => ToolCallContext::default(),
        }
    }
}

fn assistant_turn_history_message(content: Vec<ContentBlock>) -> Result<Option<Message>> {
    let has_visible_payload = content.iter().any(|block| match block {
        ContentBlock::Text { text } => !text.trim().is_empty(),
        ContentBlock::ToolUse { .. } => true,
        ContentBlock::ProviderMetadata { .. } => false,
    });
    if !has_visible_payload {
        return Ok(None);
    }
    Ok(Some(Message {
        role: "assistant".to_string(),
        content: serde_json::to_value(&content)?,
    }))
}

fn missing_proposed_plan_error() -> String {
    "Error: exit_plan_mode requires a proposed plan. Emit a <proposed_plan> block before calling exit_plan_mode.".to_string()
}

fn incomplete_proposed_plan_error() -> String {
    "Error: exit_plan_mode requires a complete <proposed_plan>...</proposed_plan> block. Close the block with </proposed_plan> before calling exit_plan_mode.".to_string()
}

fn is_compact_boundary_message(message: &Message) -> bool {
    message.role == "system"
        && message.content.get("type").and_then(Value::as_str) == Some("compact_boundary")
}

fn recoverable_runtime_error_kind(err: &anyhow::Error) -> Option<&'static str> {
    for cause in err.chain() {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            return match io_err.kind() {
                std::io::ErrorKind::PermissionDenied => Some("permission_denied"),
                std::io::ErrorKind::NotFound => Some("path_not_found"),
                std::io::ErrorKind::AlreadyExists => Some("path_already_exists"),
                std::io::ErrorKind::Interrupted => Some("interrupted"),
                std::io::ErrorKind::WouldBlock => Some("would_block"),
                std::io::ErrorKind::WriteZero => Some("write_zero"),
                std::io::ErrorKind::UnexpectedEof => Some("unexpected_eof"),
                std::io::ErrorKind::StorageFull => Some("storage_full"),
                _ => {
                    let text = io_err.to_string().to_ascii_lowercase();
                    if text.contains("operation not permitted") {
                        Some("operation_not_permitted")
                    } else {
                        Some("io_error")
                    }
                }
            };
        }
    }
    let text = err.to_string().to_ascii_lowercase();
    if text.contains("no space left on device") {
        Some("storage_full")
    } else if text.contains("sandbox") || text.contains("operation not permitted") {
        Some("operation_not_permitted")
    } else {
        None
    }
}

fn recoverable_runtime_error_message(kind: &str, err: &anyhow::Error) -> Message {
    let error = redact_secrets(
        err.chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\ncaused by: "),
    );
    Message {
        role: "user".to_string(),
        content: json!([{"type": "text", "text": format!(
            "<agent_runtime_error>\nkind: {kind}\nerror:\n{error}\n\ninstructions:\n- Treat this as a recoverable local runtime or filesystem error from the previous step.\n- Explain the likely cause briefly, then choose the safest next action.\n- If the error came from disk space, sandboxing, or file permissions, inspect or suggest remediation instead of repeating the exact failing operation blindly.\n- Continue the same user task when it is safe to do so.\n</agent_runtime_error>"
        )}]),
    }
}
