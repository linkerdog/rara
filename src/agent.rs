mod compact;
mod context_view;
mod control_handler;
mod memory_retrieval;
mod planning;
mod prompting;
#[cfg(test)]
mod tests;

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Context, Result};
use rara_instructions::HookLifecycle;
use rara_memory::memory_handle::MemoryHandle;
use rara_persistence::redaction::redact_secrets;
use rara_state::state_db::StateDb;
use rara_tools::planning::{ENTER_PLAN_MODE_TOOL_NAME, EXIT_PLAN_MODE_TOOL_NAME};
use rara_tools::tool::ToolOutputStream;
use rara_tools::tool::{ToolCallContext, ToolManager, ToolProgressEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::context::{
    AgentTurnTraceView, FileSearchCandidateProvider, RetrievalCandidate, RetrievedMemoryCandidate,
};
use crate::control_tokens::scrub_internal_control_tokens;
use crate::hook_runtime::HookRuntime;
use crate::hooks::HookDefinition;
use crate::hooks::HookParseStatus;
use crate::hooks::HookRegistry;
use crate::hooks::{HookOutcome, HookSandbox, run_sandboxed_hook};
use crate::llm::{ContentBlock, LlmBackend, LlmStreamEvent, LlmTurnMetadata};
use crate::lsp_manager::LspManager;
use crate::mcp_status::McpStatusSnapshot;
use crate::memory_notice::memory_notice;
use crate::memory_store::MemoryStore;
use crate::prompt::{self, PromptMode, PromptRuntimeConfig};
use crate::protocol_sources::{PromptSourceRegistry, SkillSourceRegistry};
use crate::session::SessionManager;
use crate::tasklist::DEFAULT_TASK_LIST_ID;
use crate::thread_store::ThreadRecorder;
use crate::todo::TodoState;
use crate::tool_result::{
    ToolResultProjectionPolicy, ToolResultProjectionReport, ToolResultStore,
    default_tool_result_store_dir, enforce_tool_result_batch_budget,
    project_tool_results_for_context, repair_tool_result_history,
};
use crate::tools::agent::{AgentDefinitionCache, AgentDefinitionLoadRecord};
use crate::tools::bash::BashCommandInput;
use crate::tools::todo::TODO_WRITE_TOOL_NAME;
use crate::workspace::WorkspaceMemory;

const MAX_RUNTIME_ERROR_RECOVERY_ATTEMPTS: usize = 1;
const MAX_PLAN_EXIT_REPAIR_ATTEMPTS: usize = 1;
const MAX_STOP_HOOK_CONTINUATIONS: usize = 8;

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

pub use rara_core::llm::types::Message;

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
    MemoryAction {
        message: String,
    },
    McpStatusUpdated(McpStatusSnapshot),
    McpStatusLoadFailed {
        message: String,
    },
    TodoUpdated(TodoState),
    /// The execution plan changed and should be rendered as a complete snapshot.
    PlanUpdated {
        steps: Vec<PlanStep>,
        explanation: Option<String>,
    },
    /// A structured approval is required before the agent can continue.
    ApprovalRequested {
        approval_id: String,
        kind: String,
    },
    /// A structured approval decision was applied.
    ApprovalAnswered {
        approval_id: String,
        approved: bool,
    },
    /// Agent loop started a new run.
    AgentStart,
    /// Agent loop stopped normally (e.g. turn complete, user interruption).
    AgentStop {
        reason: String,
    },
    /// Agent error that may be recoverable (retry) or terminal.
    AgentError {
        message: String,
        recoverable: bool,
    },
    /// Agent is about to call the model with accumulated history.
    ModelRequest {
        model: String,
        input_tokens: u32,
    },
    /// Model returned a complete response for this stream.
    ModelResponse {
        model: String,
        output_tokens: u32,
        finish_reason: Option<String>,
    },
    /// Context history was compacted and the resulting session projection is
    /// available to presentation clients.
    Compaction {
        count: usize,
        before_tokens: usize,
        after_tokens: usize,
        summary: String,
        recent_files: Vec<String>,
    },
}

#[derive(Debug)]
struct ToolCall {
    id: String,
    name: String,
    input: Value,
}

#[derive(Debug, PartialEq, Eq)]
struct StopHookBlock {
    hook_id: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct StopHookOutput {
    decision: Option<String>,
    reason: Option<String>,
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: Option<StopHookSpecificOutput>,
}

#[derive(Debug, Deserialize)]
struct StopHookSpecificOutput {
    #[serde(rename = "additionalContext")]
    additional_context: Option<String>,
}

fn message_text(message: &Message) -> Option<String> {
    match &message.content {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    block
                        .as_str()
                        .or_else(|| block.get("text").and_then(Value::as_str))
                })
                .collect::<String>();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn stop_hook_block_reason(outcome: &HookOutcome) -> Option<String> {
    if outcome.exit_code == Some(2) {
        return Some(non_empty_hook_message(&outcome.stderr));
    }
    if outcome.exit_code != Some(0) {
        return None;
    }
    let output: StopHookOutput = serde_json::from_str(&outcome.stdout).ok()?;
    if output.decision.as_deref() == Some("block") {
        return Some(
            output
                .reason
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or_else(|| "Stop hook blocked completion.".to_string()),
        );
    }
    output
        .hook_specific_output
        .and_then(|output| output.additional_context)
        .filter(|context| !context.trim().is_empty())
}

fn non_empty_hook_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        "Stop hook exited with code 2.".to_string()
    } else {
        message.to_string()
    }
}

fn stop_hook_feedback(block: &StopHookBlock) -> Message {
    Message {
        role: "system".to_string(),
        content: Value::String(format!(
            "Stop hook {} blocked completion: {}\nResolve the reported condition before finishing.",
            block.hook_id, block.reason
        )),
    }
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
    streamed_text_delta: bool,
    streamed_reasoning_delta: bool,
    model_stop_reason: Option<String>,
}

pub struct Agent {
    pub tool_manager: ToolManager,
    pub llm_backend: Arc<dyn LlmBackend>,
    pub memory_handle: Arc<MemoryHandle>,
    pub memory_store: Arc<MemoryStore>,
    pub session_manager: Arc<SessionManager>,
    pub consolidation_scheduler: rara_memory::consolidation::ConsolidationScheduler,
    state_db: Option<Arc<StateDb>>,
    pub workspace: Arc<WorkspaceMemory>,
    pub history: Vec<Message>,
    pub session_id: String,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub total_cache_hit_tokens: u32,
    pub total_cache_miss_tokens: u32,
    pub aux_total_cache_hit_tokens: u32,
    pub aux_total_cache_miss_tokens: u32,
    pub tool_result_store: ToolResultStore,
    pub max_turns: Option<usize>,
    pub token_budget: Option<u32>,
    pub token_budget_exhausted: bool,
    pub execution_mode: AgentExecutionMode,
    pub bash_approval_mode: BashApprovalMode,
    pub full_access_mode: bool,
    pub current_plan: Vec<PlanStep>,
    pub plan_explanation: Option<String>,
    pub pending_user_input: Option<PendingUserInput>,
    pub pending_approval: Option<PendingApproval>,
    pub todo_state: Option<TodoState>,
    pub task_list_id: String,
    pub agent_definitions: AgentDefinitionCache,
    pub completed_user_input: Option<CompletedInteraction>,
    pub completed_approval: Option<CompletedInteraction>,
    pub approved_bash_prefixes: Vec<String>,
    pub compact_state: CompactState,
    pub hook_registry: Option<Arc<crate::hooks::HookRegistry>>,
    pub hook_sandbox: Option<HookSandbox>,
    hook_runtime: Option<Arc<HookRuntime>>,
    plugin_hook_runtime: Option<Arc<crate::plugin_middleware::PluginHookRuntime>>,
    plugin_session_start_hooks_ran: bool,
    pub retrieved_memory_candidates: Vec<RetrievedMemoryCandidate>,
    pub file_search_candidates: Vec<RetrievalCandidate>,
    pub mcp_resource_candidates: Vec<RetrievalCandidate>,
    pub hook_output_candidates: Vec<RetrievalCandidate>,
    pub graph_context_candidates: Vec<RetrievalCandidate>,
    pub last_tool_result_projection_report: ToolResultProjectionReport,
    pub last_agent_turn_trace: AgentTurnTraceView,
    file_search_provider: FileSearchCandidateProvider,
    inspection_progress: InspectionProgress,
    last_query_plan_updated: bool,
    recent_tool_calls: Vec<(String, String)>,
    pending_plan_exit_tool_id: Option<String>,
    prompt_config: PromptRuntimeConfig,
    prompt_source_registry: Option<Arc<PromptSourceRegistry>>,
    skill_source_registry: Option<Arc<SkillSourceRegistry>>,
    lsp_manager: Option<Arc<LspManager>>,
    cancellation_token: Option<Arc<AtomicBool>>,
    last_interaction_time: std::time::Instant,
}

impl Agent {
    /// Configure hook execution context. Called after construction.
    pub fn set_hook_context(
        &mut self,
        registry: Arc<crate::hooks::HookRegistry>,
        sandbox: HookSandbox,
        runtime: Arc<HookRuntime>,
    ) {
        self.hook_registry = Some(registry);
        self.hook_sandbox = Some(sandbox);
        self.hook_runtime = Some(runtime);
    }

    /// Configure Claude plugin hooks loaded for this runtime session.
    pub(crate) fn set_plugin_hook_runtime(
        &mut self,
        runtime: Arc<crate::plugin_middleware::PluginHookRuntime>,
    ) {
        self.plugin_hook_runtime = Some(runtime);
    }

    pub(crate) fn plugin_command_count(&self) -> usize {
        self.plugin_hook_runtime
            .as_ref()
            .map_or(0, |runtime| runtime.command_summaries().len())
    }

    /// Accumulate subagent (auxiliary model) cache statistics.
    /// Called by consolidation and other subagent completion
    /// handlers to split cache reporting between main and aux models.
    pub fn accumulate_aux_cache(&mut self, hit: u32, miss: u32) {
        self.aux_total_cache_hit_tokens += hit;
        self.aux_total_cache_miss_tokens += miss;
    }

    #[cfg(test)]
    pub fn new(
        tool_manager: ToolManager,
        llm_backend: Arc<dyn LlmBackend>,
        memory_handle: Arc<MemoryHandle>,
        session_manager: Arc<SessionManager>,
        workspace: Arc<WorkspaceMemory>,
    ) -> Self {
        let agent_definitions = AgentDefinitionCache::load(workspace.root.clone());
        Self::new_with_agent_definitions(
            tool_manager,
            llm_backend,
            memory_handle,
            session_manager,
            workspace,
            agent_definitions,
        )
    }

    pub fn new_with_agent_definitions(
        tool_manager: ToolManager,
        llm_backend: Arc<dyn LlmBackend>,
        memory_handle: Arc<MemoryHandle>,
        session_manager: Arc<SessionManager>,
        workspace: Arc<WorkspaceMemory>,
        agent_definitions: AgentDefinitionCache,
    ) -> Self {
        let root = workspace.root.clone();
        let memory_store = Arc::new(MemoryStore::new_with_handle(
            llm_backend.clone(),
            memory_handle.clone(),
        ));
        let state_db =
            session_manager.storage_dir.parent().and_then(
                |rara_dir| match StateDb::new_for_root_dir(rara_dir.to_path_buf()) {
                    Ok(state_db) => Some(Arc::new(state_db)),
                    Err(err) => {
                        eprintln!(
                            "Warning: could not initialize session state db at {}: {err}",
                            rara_dir.display()
                        );
                        None
                    }
                },
            );
        let memory_root = if let Some(rara_dir) = session_manager.storage_dir.parent() {
            rara_dir.join("memory")
        } else {
            workspace.root.join(".rara").join("memory")
        };
        let consolidation_config = rara_memory::consolidation::ConsolidationConfig::default();
        let consolidation_scheduler = rara_memory::consolidation::ConsolidationScheduler::new(
            memory_root,
            consolidation_config,
        );
        Self {
            tool_manager,
            llm_backend,
            memory_handle,
            memory_store,
            session_manager,
            consolidation_scheduler,
            state_db,
            workspace,
            history: Vec::new(),
            session_id: Uuid::new_v4().to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_hit_tokens: 0,
            total_cache_miss_tokens: 0,
            aux_total_cache_hit_tokens: 0,
            aux_total_cache_miss_tokens: 0,
            tool_result_store: ToolResultStore::new(
                default_tool_result_store_dir().unwrap_or_else(|_| {
                    std::env::temp_dir().join(format!("rara-tool-results-{}", Uuid::new_v4()))
                }),
            )
            .unwrap_or_else(|err| {
                eprintln!("Warning: could not create tool result store: {err}");
                ToolResultStore::new(std::env::temp_dir().join("rara-fallback")).unwrap_or_else(
                    |_| {
                        // Absolute last resort: use a /tmp subdir that should always work
                        ToolResultStore::new(format!("/tmp/rara-tool-results-{}", Uuid::new_v4()))
                            .expect("unrecoverable: cannot create tool result store")
                    },
                )
            }),
            execution_mode: AgentExecutionMode::Execute,
            max_turns: None,
            token_budget: None,
            token_budget_exhausted: false,
            bash_approval_mode: BashApprovalMode::Always,
            full_access_mode: false,
            current_plan: Vec::new(),
            plan_explanation: None,
            pending_user_input: None,
            pending_approval: None,
            todo_state: None,
            task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
            agent_definitions,
            completed_user_input: None,
            completed_approval: None,
            approved_bash_prefixes: Vec::new(),
            compact_state: CompactState::default(),
            hook_registry: None,
            hook_sandbox: None,
            hook_runtime: None,
            plugin_hook_runtime: None,
            plugin_session_start_hooks_ran: false,
            retrieved_memory_candidates: Vec::new(),
            file_search_candidates: Vec::new(),
            mcp_resource_candidates: Vec::new(),
            hook_output_candidates: Vec::new(),
            graph_context_candidates: Vec::new(),
            last_tool_result_projection_report: ToolResultProjectionReport::default(),
            last_agent_turn_trace: AgentTurnTraceView::default(),
            file_search_provider: FileSearchCandidateProvider::new(root, true),
            inspection_progress: InspectionProgress::default(),
            last_query_plan_updated: false,
            recent_tool_calls: Vec::new(),
            pending_plan_exit_tool_id: None,
            prompt_config: PromptRuntimeConfig::default(),
            prompt_source_registry: None,
            skill_source_registry: None,
            lsp_manager: None,
            cancellation_token: None,
            last_interaction_time: std::time::Instant::now(),
        }
    }

    pub async fn query(&mut self, prompt: String) -> Result<()> {
        self.query_with_mode(prompt, AgentOutputMode::Terminal)
            .await
    }

    pub fn agent_definition_records(&self) -> Vec<AgentDefinitionLoadRecord> {
        self.agent_definitions.records()
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
        self.last_interaction_time = std::time::Instant::now();
        let mut agentic_turns = 0usize;
        let mut runtime_error_recoveries = 0usize;
        self.inspection_progress = InspectionProgress::default();
        self.last_query_plan_updated = false;
        self.pending_plan_exit_tool_id = None;
        self.run_plugin_session_start_hooks_once().await;
        self.run_user_prompt_submit_plugin_hooks(&prompt).await;
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
        report(AgentEvent::MemoryAction {
            message: memory_notice("querying workspace memory"),
        });
        self.refresh_memory_retrieval_candidates().await;
        report(AgentEvent::MemoryAction {
            message: memory_notice(
                self.workspace
                    .memory_notice_text(self.retrieved_memory_candidates.len()),
            ),
        });
        self.refresh_file_search_candidates();
        self.refresh_protocol_prompt_sources_for_query().await;
        self.refresh_protocol_skill_sources_for_query().await;

        match self
            .run_agent_loop_with_limit(output_mode, &mut report, &mut agentic_turns)
            .await
        {
            Ok(()) => {
                // Post-turn consolidation check (fire-and-forget).
                let sessions = self.consolidation_scheduler.check();
                if sessions.is_some() {
                    let prompt_config = self.prompt_config.clone();
                    let llm_backend = self.llm_backend.clone();
                    let memory_handle = self.memory_handle.clone();
                    let session_manager = self.session_manager.clone();
                    let workspace = self.workspace.clone();
                    let scheduler = self.consolidation_scheduler.clone();
                    let task_list_id = self.task_list_id.clone();
                    let agent_definitions = self.agent_definitions.clone();
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("consolidation runtime");
                        rt.block_on(async move {
                            let Some(sessions) = sessions else { return };
                            let Some(_lock) = scheduler.acquire_lock() else {
                                return;
                            };
                            let prompt =
                                rara_memory::dream_prompts::build_consolidation_prompt(&sessions);
                            eprintln!(
                                "consolidation: {} sessions ready, dispatching subagent",
                                sessions.len()
                            );
                            let result = crate::tools::agent::run_sub_agent(
                                crate::tools::agent::SubAgentKind::Consolidate,
                                &uuid::Uuid::new_v4().to_string(),
                                None,
                                Some("consolidation"),
                                None,
                                &prompt,
                                None,
                                None,
                                None,
                                llm_backend,
                                Arc::new(crate::tools::agent::InheritedSubagentBackendResolver),
                                memory_handle,
                                session_manager,
                                workspace,
                                prompt_config,
                                task_list_id,
                                agent_definitions,
                                None,
                            )
                            .await;
                            match result {
                                Ok(r) => {
                                    let line = if r.summary.is_empty() {
                                        format!(
                                            "📝 consolidation complete — status={} (cache: {}/{} hit/miss)",
                                            r.status, r.total_cache_hit_tokens, r.total_cache_miss_tokens
                                        )
                                    } else {
                                        format!(
                                            "📝 consolidation: {} (cache: {}/{} hit/miss)",
                                            r.summary, r.total_cache_hit_tokens, r.total_cache_miss_tokens
                                        )
                                    };
                                    eprintln!("{}", line);
                                }
                                Err(e) => eprintln!("consolidation subagent failed: {e}"),
                            }
                        });
                    });
                }
            }
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
        let session_manager = self.session_manager.clone();
        let session_id = self.session_id.clone();
        let save_result = tokio::task::spawn_blocking(move || {
            session_manager.save_session_context_checkpoint(
                &session_id,
                turn_start_idx as u32,
                turn_text,
            )
        })
        .await;
        if matches!(save_result, Ok(Ok(()))) {
            report(AgentEvent::MemoryAction {
                message: memory_notice("wrote session checkpoint"),
            });
        }
        Ok(())
    }

    pub(super) fn checkpoint_session(&self) -> Result<()> {
        if let Some(state_db) = self.state_db.as_deref() {
            let recorder = ThreadRecorder::new(state_db);
            return recorder.persist_history_checkpoint(&self.session_id, &self.history);
        }
        self.session_manager
            .save_session(&self.session_id, &self.history)
            .context("save session without state db")
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
        let history_for_query = self
            .history
            .iter()
            .filter(|message| !is_compact_boundary_message(message))
            .cloned()
            .collect::<Vec<_>>();
        let projection_policy = self.tool_result_projection_policy();
        let (mut messages, projection_report) =
            project_tool_results_for_context(&history_for_query, &projection_policy);
        self.last_tool_result_projection_report = projection_report.clone();
        if projection_report.cleared_results > 0 {
            report(AgentEvent::Status(format!(
                "Projected {} old tool result(s) out of this model request.",
                projection_report.cleared_results
            )));
        }
        let mut system_content = Vec::new();
        if let Some(_index) = assembled.prompt.effective_prompt.dynamic_boundary_index {
            let full_text = &assembled.prompt.effective_prompt.text;
            // The text is joined by "\n\n". We want to split it back or just use the sections.
            // But EffectivePrompt only gives us the full text and boundary index.
            // Actually, build_effective_prompt joins them.

            let parts: Vec<&str> = full_text
                .split(rara_instructions::DYNAMIC_BOUNDARY)
                .collect();
            if parts.len() >= 2 {
                let static_part = parts[0].trim();
                let dynamic_part = parts[1..].join(rara_instructions::DYNAMIC_BOUNDARY);
                let dynamic_part = dynamic_part.trim();

                if !static_part.is_empty() {
                    system_content.push(json!({
                        "type": "text",
                        "text": static_part,
                        "cache_control": {"type": "ephemeral"} // Add hint for Anthropic-style caching
                    }));
                }
                // Add the boundary itself if needed or just skip it.
                // Claude Code keeps it to mark the boundary for future edits.
                system_content.push(json!({
                    "type": "text",
                    "text": rara_instructions::DYNAMIC_BOUNDARY,
                }));
                if !dynamic_part.is_empty() {
                    system_content.push(json!({
                        "type": "text",
                        "text": dynamic_part,
                    }));
                }
            } else {
                system_content.push(json!(assembled.prompt.effective_prompt.text));
            }
        } else {
            system_content.push(json!(assembled.prompt.effective_prompt.text));
        }

        messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: if system_content.len() == 1 {
                    system_content.remove(0)
                } else {
                    Value::Array(system_content)
                },
            },
        );
        if let Some(memory_context) = Agent::selected_memory_context_text(&assembled.runtime) {
            Agent::prepend_memory_context_to_latest_user_message(&mut messages, memory_context);
        }

        let model_label = self.model_event_label();
        report(AgentEvent::ModelRequest {
            model: model_label.clone(),
            // Provider token usage is only available after the response.
            // RuntimeControl documents 0 here as the unknown-count sentinel.
            input_tokens: 0,
        });

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
                        streamed_any_reasoning_delta = true;
                        report(AgentEvent::AssistantThinkingDelta(delta));
                    }
                }
            })
            .await?;

        let output_tokens = response
            .usage
            .as_ref()
            .map(|usage| usage.output_tokens)
            .unwrap_or(0);
        report(AgentEvent::ModelResponse {
            model: model_label,
            output_tokens,
            finish_reason: response.stop_reason.clone(),
        });

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
                            if self.capture_plan_from_text(&clean_text)? {
                                plan_updated = true;
                                report(AgentEvent::PlanUpdated {
                                    steps: self.current_plan.clone(),
                                    explanation: self.plan_explanation.clone(),
                                });
                            }
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
                        && let Some((steps, explanation)) =
                            planning::parse_exit_plan_tool_input(input)
                    {
                        self.current_plan = steps;
                        self.plan_explanation = explanation;
                        plan_updated = true;
                        report(AgentEvent::PlanUpdated {
                            steps: self.current_plan.clone(),
                            explanation: self.plan_explanation.clone(),
                        });
                    }
                    sanitized_content.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                    let modified_input = match self.hook_runtime.as_ref() {
                        Some(runtime) => runtime.modify_tool_input(name.as_str(), input.clone()),
                        None => input.clone(),
                    };
                    report(AgentEvent::ToolUse {
                        name: name.clone(),
                        input: modified_input.clone(),
                    });
                    tool_calls.push(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: modified_input,
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
            streamed_text_delta: streamed_any_text_delta,
            streamed_reasoning_delta: streamed_any_reasoning_delta,
            model_stop_reason: response.stop_reason,
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

    fn model_event_label(&self) -> String {
        self.llm_backend
            .model_label()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string())
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

    /// Post-turn consolidation check (Claude Code style).
    ///
    /// Checks whether memory consolidation is due.  When sessions are
    /// ready, acquires the lock and dispatches a Consolidate subagent
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
        let mut stop_hook_continuations = 0usize;
        let mut session_end_last_assistant_message: Option<String> = None;
        let mut should_run_session_end_hooks = false;
        loop {
            if let Some(max) = self.max_turns
                && *agentic_turns >= max
            {
                self.last_agent_turn_trace.loop_outcome = Some("stopped".to_string());
                self.last_agent_turn_trace.continuation_phase =
                    Some("max_turns_reached".to_string());
                report(AgentEvent::Status(format!(
                    "Agent reached max-turns limit ({max})",
                )));
                session_end_last_assistant_message = self.latest_assistant_message_text();
                should_run_session_end_hooks = true;
                break;
            }
            if let Some(budget) = self.token_budget
                && self.total_model_tokens() >= budget
            {
                self.token_budget_exhausted = true;
                self.last_agent_turn_trace.loop_outcome = Some("stopped".to_string());
                self.last_agent_turn_trace.continuation_phase =
                    Some("token_budget_exhausted".to_string());
                report(AgentEvent::Status(format!(
                    "Agent reached token budget ({}/{budget})",
                    self.total_model_tokens()
                )));
                session_end_last_assistant_message = self.latest_assistant_message_text();
                should_run_session_end_hooks = true;
                break;
            }
            self.ensure_active_plan_step();
            // Inject hook outputs as system messages before the model turn
            self.hook_output_candidates.clear();
            if let Some(hr) = self.hook_runtime.as_ref() {
                let outputs = hr.blocking_drain_outputs();
                self.hook_output_candidates = outputs
                    .iter()
                    .enumerate()
                    .map(|(index, text)| hook_output_candidate(text, index, &self.session_id))
                    .collect();
                for text in outputs {
                    self.history.push(Message {
                        role: "system".to_string(),
                        content: Value::String(text),
                    });
                }
            }
            let mut turn_output = match self.run_model_turn(output_mode, report).await {
                Ok(turn_output) => turn_output,
                Err(err) if is_interrupt_error(&err) => {
                    self.run_session_end_plugin_hooks(self.latest_assistant_message_text(), true)
                        .await;
                    return Err(err);
                }
                Err(err) => return Err(err),
            };
            self.record_agent_turn_trace(&turn_output, *agentic_turns, None, None, false);
            self.last_query_plan_updated = turn_output.plan_updated;
            if !turn_output.tool_calls.is_empty() {
                // Detect repeated tool calls — both within a single turn
                // and across consecutive turns.
                let mut identical_calls_within_turn = 0;
                let candidates: Vec<(String, String)> = turn_output
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        let input_key = serde_json::to_string(&tc.input).unwrap_or_default();
                        (tc.name.clone(), input_key)
                    })
                    .collect();
                let prev = &self.recent_tool_calls;
                let dup_count = candidates.iter().filter(|c| prev.contains(c)).count();
                // Also check within-turn repeats
                if candidates.len() >= 2 {
                    for i in 1..candidates.len() {
                        if candidates[i] == candidates[i - 1] {
                            identical_calls_within_turn += 1;
                        }
                    }
                }
                self.recent_tool_calls = candidates;
                if dup_count >= 2 || identical_calls_within_turn >= 1 {
                    report(AgentEvent::Status(
                        "Repeated tool call pattern detected. Consider re-evaluating the approach."
                            .to_string(),
                    ));
                }
            }
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
                    self.record_agent_turn_trace(
                        &turn_output,
                        *agentic_turns,
                        Some("continued"),
                        Some(RuntimeContinuationPhase::PlanExitRepairRequired.label()),
                        false,
                    );
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
                self.record_agent_turn_trace(
                    &turn_output,
                    *agentic_turns,
                    Some("stopped"),
                    Some("plan_exit_repair_exhausted"),
                    false,
                );
                self.checkpoint_session()?;
                session_end_last_assistant_message = self.latest_assistant_message_text();
                should_run_session_end_hooks = true;
                break;
            }
            let last_assistant_message = turn_output
                .assistant_message
                .as_ref()
                .and_then(message_text);
            let assistant_message_recorded = turn_output.assistant_message.is_some();
            if let Some(message) = turn_output.assistant_message.take() {
                self.push_history_message(message);
                self.checkpoint_session()?;
            }
            self.record_agent_turn_trace(
                &turn_output,
                *agentic_turns,
                None,
                None,
                assistant_message_recorded,
            );

            if turn_output.tool_calls.is_empty() {
                let is_reasoning_only = Self::is_reasoning_only_turn(
                    turn_output.had_text_response,
                    turn_output.had_reasoning_response,
                );
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
                    let phase = if is_reasoning_only {
                        RuntimeContinuationPhase::ReasoningOnlyContinuationRequired
                    } else {
                        RuntimeContinuationPhase::PlanContinuationRequired
                    };
                    self.record_agent_turn_trace(
                        &turn_output,
                        *agentic_turns,
                        Some("continued"),
                        Some(phase.label()),
                        assistant_message_recorded,
                    );
                    self.push_history_message(
                        self.runtime_continuation_message(phase, *agentic_turns),
                    );
                    self.checkpoint_session()?;
                    continue;
                }
                if self.should_continue_execute_without_tools(
                    turn_output.continue_inspection,
                    turn_output.had_text_response,
                    turn_output.had_reasoning_response,
                ) {
                    let phase = if is_reasoning_only {
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
                    self.record_agent_turn_trace(
                        &turn_output,
                        *agentic_turns,
                        Some("continued"),
                        Some(phase.label()),
                        assistant_message_recorded,
                    );
                    self.push_history_message(
                        self.runtime_continuation_message(phase, *agentic_turns),
                    );
                    self.checkpoint_session()?;
                    continue;
                }
                if let Some(block) = self.run_stop_hooks(
                    last_assistant_message.as_deref(),
                    stop_hook_continuations > 0,
                    report,
                ) {
                    if stop_hook_continuations < MAX_STOP_HOOK_CONTINUATIONS {
                        stop_hook_continuations += 1;
                        *agentic_turns += 1;
                        report(AgentEvent::AgentError {
                            message: format!(
                                "Stop hook {} blocked completion: {}",
                                block.hook_id, block.reason
                            ),
                            recoverable: true,
                        });
                        report(AgentEvent::Status(
                            "Stop hook blocked completion. Continuing with hook feedback."
                                .to_string(),
                        ));
                        self.record_agent_turn_trace(
                            &turn_output,
                            *agentic_turns,
                            Some("continued"),
                            Some("stop_hook_blocked"),
                            assistant_message_recorded,
                        );
                        self.push_history_message(stop_hook_feedback(&block));
                        self.checkpoint_session()?;
                        continue;
                    }
                    report(AgentEvent::AgentError {
                        message: format!(
                            "Stop hook {} continued to block after {MAX_STOP_HOOK_CONTINUATIONS} attempts; allowing completion.",
                            block.hook_id
                        ),
                        recoverable: false,
                    });
                }
                self.record_agent_turn_trace(
                    &turn_output,
                    *agentic_turns,
                    Some("stopped"),
                    Some("final_no_tool_response"),
                    assistant_message_recorded,
                );
                self.complete_active_plan_step();
                session_end_last_assistant_message = last_assistant_message;
                should_run_session_end_hooks = true;
                break;
            }
            *agentic_turns += 1;
            self.record_agent_turn_trace(
                &turn_output,
                *agentic_turns,
                Some("running_tools"),
                Some("tool_calls_available"),
                assistant_message_recorded,
            );

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
        if should_run_session_end_hooks {
            self.run_session_end_plugin_hooks(session_end_last_assistant_message, false)
                .await;
        }
        Ok(())
    }

    async fn run_session_end_plugin_hooks(
        &self,
        last_assistant_message: Option<String>,
        is_interrupt: bool,
    ) {
        if let Some(plugin_hooks) = self.plugin_hook_runtime.clone() {
            plugin_hooks
                .run_session_end(last_assistant_message.as_deref(), is_interrupt)
                .await;
        }
    }

    async fn run_plugin_session_start_hooks_once(&mut self) {
        if self.plugin_session_start_hooks_ran {
            return;
        }
        if let Some(plugin_hooks) = self.plugin_hook_runtime.clone() {
            self.plugin_session_start_hooks_ran = true;
            plugin_hooks.run_session_start().await;
        }
    }

    async fn run_user_prompt_submit_plugin_hooks(&self, prompt: &str) {
        if let Some(plugin_hooks) = self.plugin_hook_runtime.clone() {
            plugin_hooks.run_user_prompt_submit(prompt).await;
        }
    }

    fn latest_assistant_message_text(&self) -> Option<String> {
        self.history
            .iter()
            .rev()
            .find(|message| message.role == "assistant")
            .and_then(message_text)
    }

    fn run_stop_hooks<F>(
        &self,
        last_assistant_message: Option<&str>,
        stop_hook_active: bool,
        report: &mut F,
    ) -> Option<StopHookBlock>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let (Some(registry), Some(sandbox)) = (&self.hook_registry, &self.hook_sandbox) else {
            return None;
        };
        let input = json!({
            "session_id": self.session_id,
            "cwd": sandbox.workspace_root,
            "hook_event_name": "Stop",
            "stop_hook_active": stop_hook_active,
            "last_assistant_message": last_assistant_message.unwrap_or_default(),
        })
        .to_string();

        for hook in registry.executable_hooks_for_phase(HookLifecycle::Stop) {
            match run_sandboxed_hook(hook, sandbox, &input) {
                Ok(outcome) => {
                    if outcome.timed_out {
                        report(AgentEvent::Status(format!(
                            "Stop hook {} timed out; allowing completion.",
                            hook.id
                        )));
                        continue;
                    }
                    if let Some(reason) = stop_hook_block_reason(&outcome) {
                        return Some(StopHookBlock {
                            hook_id: hook.id.clone(),
                            reason,
                        });
                    }
                    if outcome.exit_code.is_some_and(|code| code != 0) {
                        report(AgentEvent::Status(format!(
                            "Stop hook {} exited unsuccessfully; allowing completion: {}",
                            hook.id,
                            outcome.stderr.trim()
                        )));
                    }
                }
                Err(error) => report(AgentEvent::Status(format!(
                    "Stop hook {} failed; allowing completion: {error}",
                    hook.id
                ))),
            }
        }
        None
    }

    fn record_agent_turn_trace(
        &mut self,
        turn_output: &TurnOutput,
        agentic_turn_index: usize,
        loop_outcome: Option<&str>,
        continuation_phase: Option<&str>,
        assistant_message_recorded: bool,
    ) {
        let reasoning_only = Self::is_reasoning_only_turn(
            turn_output.had_text_response,
            turn_output.had_reasoning_response,
        );
        self.last_agent_turn_trace = AgentTurnTraceView {
            agentic_turn_index,
            execution_mode: self.execution_mode_label().to_string(),
            model_stop_reason: turn_output.model_stop_reason.clone(),
            loop_outcome: loop_outcome.map(ToString::to_string),
            continuation_phase: continuation_phase.map(ToString::to_string),
            had_text_response: turn_output.had_text_response,
            had_reasoning_response: turn_output.had_reasoning_response,
            reasoning_only,
            streamed_text_delta: turn_output.streamed_text_delta,
            streamed_reasoning_delta: turn_output.streamed_reasoning_delta,
            assistant_message_recorded,
            tool_call_count: turn_output.tool_calls.len(),
            plan_updated: turn_output.plan_updated,
            continue_inspection: turn_output.continue_inspection,
            malformed_proposed_plan: turn_output.malformed_proposed_plan,
        };
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
                report(AgentEvent::ApprovalRequested {
                    approval_id: self
                        .pending_plan_exit_tool_id
                        .clone()
                        .expect("plan approval id was just assigned"),
                    kind: "plan".to_string(),
                });
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
                && !request.is_read_only()
            {
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
            if let Some(request) = bash_request.as_ref()
                && !self.full_access_mode
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
                    report(AgentEvent::ApprovalRequested {
                        approval_id: tool_id.clone(),
                        kind: "shell".to_string(),
                    });
                    report(AgentEvent::Status(
                        "Bash approval required. Waiting for a structured user decision."
                            .to_string(),
                    ));
                    break;
                }
            }
            // ── Auto-permission classifier safety net ────────────────────────────
            // Safety net: for dangerous tools (bash, web_*, pty), run the LLM
            // classifier to detect suspicious commands the static rules missed.
            // Explicit full access delegates that boundary to the caller's
            // external isolation and therefore bypasses this local gate.
            const CLASSIFIABLE_TOOLS: &[&str] =
                &["bash", "pty", "web_search", "web_fetch", "mcp_tool_search"];
            if !self.full_access_mode && CLASSIFIABLE_TOOLS.contains(&tool_name.as_str()) {
                let classifier_input = tool_input.clone();
                let request = crate::classifier::AutoPermissionRequest {
                    tool_name: tool_name.clone(),
                    tool_input: classifier_input,
                    workspace_hint: Some(self.workspace.root.display().to_string()),
                };
                match self.classify_auto_permission(&request).await {
                    Ok(resp) => {
                        report(AgentEvent::Status(format!(
                            "Auto-permission: {} — {}",
                            resp.decision, resp.reason,
                        )));
                        match resp.decision {
                            crate::classifier::AutoPermissionDecision::Deny => {
                                let error_text = format!(
                                    "Error: auto-permission classifier denied this tool call: {}",
                                    resp.reason
                                );
                                report(AgentEvent::ToolResult {
                                    name: tool_name.clone(),
                                    content: error_text.clone(),
                                    is_error: true,
                                });
                                tool_results.push(tool_result_message(&tool_id, error_text, true));
                                continue;
                            }
                            crate::classifier::AutoPermissionDecision::Allow
                            | crate::classifier::AutoPermissionDecision::Ask => {
                                // Allow — proceed to existing checks
                            }
                        }
                    }
                    Err(e) => {
                        // Classifier unavailable — fail open (existing checks remain)
                        report(AgentEvent::Status(format!(
                            "Auto-permission classifier unavailable: {e}"
                        )));
                    }
                }
            }
            // ── end auto-permission classifier ───────────────────────────────────

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
            // PreToolUse hook: run registered hooks that can allow/block.
            if let (Some(registry), Some(sandbox)) = (&self.hook_registry, &self.hook_sandbox) {
                let hooks = registry.executable_hooks_for_phase(HookLifecycle::PreToolUse);
                let mut blocked = false;
                if !hooks.is_empty() {
                    let input = serde_json::json!({
                        "tool_name": tool_name,
                        "tool_input": tool_input
                    });
                    let input_str = input.to_string();
                    for hook in &hooks {
                        match run_sandboxed_hook(hook, sandbox, &input_str) {
                            Ok(outcome) if !outcome.allows() => {
                                let msg = format!("tool {} blocked by hook {}", tool_name, hook.id);
                                tool_results.push(tool_result_message(&tool_id, msg, true));
                                if !outcome.stderr.is_empty() {
                                    eprintln!("hook {}: {}", hook.id, outcome.stderr);
                                }
                                blocked = true;
                                break;
                            }
                            Err(e) => {
                                eprintln!("hook {} failed: {}", hook.id, e);
                                blocked = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                if blocked {
                    continue;
                }
            }
            if let Some(plugin_hooks) = self.plugin_hook_runtime.clone()
                && let Some(block) = plugin_hooks.run_pre_tool_use(&tool_name, &tool_input).await
            {
                let error_text = format!(
                    "Error: tool {} blocked by plugin hook {}: {}",
                    tool_name, block.plugin_name, block.message
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
                        // Accumulate subagent (auxiliary model) cache statistics.
                        if matches!(
                            tool_name.as_str(),
                            "spawn_agent" | "explore_agent" | "plan_agent" | "team_create"
                        ) {
                            let (hit, miss) = if tool_name == "team_create" {
                                // team_create nests results under "team_results[*]".
                                result["team_results"]
                                    .as_array()
                                    .map(|results| {
                                        results.iter().fold((0, 0), |(h, m), res| {
                                            (
                                                h + res["cache_hit_tokens"].as_u64().unwrap_or(0)
                                                    as u32,
                                                m + res["cache_miss_tokens"].as_u64().unwrap_or(0)
                                                    as u32,
                                            )
                                        })
                                    })
                                    .unwrap_or((0, 0))
                            } else {
                                (
                                    result["cache_hit_tokens"].as_u64().unwrap_or(0) as u32,
                                    result["cache_miss_tokens"].as_u64().unwrap_or(0) as u32,
                                )
                            };
                            self.accumulate_aux_cache(hit, miss);
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

    /// Classify whether a tool call should be auto-allowed, denied, or requires
    /// user approval. Delegates to the LLM backend's auxiliary model.
    async fn classify_auto_permission(
        &self,
        request: &crate::classifier::AutoPermissionRequest,
    ) -> Result<crate::classifier::AutoPermissionResponse> {
        let instructions = "\
You are a security classifier. Given a user message and a proposed tool call,
output exactly one JSON object with fields:
- \"decision\": \"allow\", \"deny\", or \"ask\"
- \"reason\": a short justification
- \"matched_rule\": optional policy rule name

Rules:
- allow: read-only, safe filesystem operations within the workspace, standard build/test/lint/format commands, git status/diff/log
- deny: destructive commands (rm -rf, format disk), privilege escalation (sudo), modifying system files outside workspace, accessing sensitive paths (/etc/passwd)
- ask: network requests (curl, web_fetch), git push/commit, installing packages, modifying configs outside workspace, commands with unclear intent
        ";

        let messages = crate::classifier::build_classifier_messages(
            &self.history,
            &request.tool_name,
            &request.tool_input,
        );
        let raw = self.llm_backend.classify(instructions, &messages).await?;
        Ok(crate::classifier::parse_auto_permission_response(&raw)?)
    }

    fn tool_call_context(&self) -> ToolCallContext {
        let context = ToolCallContext::default()
            .with_session_id(self.session_id.clone())
            .with_workspace_root(self.workspace.root.clone());
        match self.cancellation_token.as_ref() {
            Some(token) => context.with_cancellation(token.clone()),
            None => context,
        }
    }
}

fn hook_output_candidate(text: &str, index: usize, session_id: &str) -> RetrievalCandidate {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update([0]);
    hasher.update((index as u64).to_le_bytes());
    hasher.update([0]);
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest.as_slice() {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    let id = format!("hook_output:{hex}");
    RetrievalCandidate {
        id: id.clone(),
        source: crate::context::RetrievalSourceRef {
            source_type: "hook_output".to_string(),
            source_id: Some(id.clone()),
            source_path: None,
            source_uri: None,
            session_id: Some(session_id.to_string()),
            thread_id: None,
            workspace_id: None,
        },
        kind: "hook_output".to_string(),
        scope: "turn".to_string(),
        label: format!("Hook Output {}", index + 1),
        detail: "drained before model turn".to_string(),
        summary: Some(text.to_string()),
        rank: index,
        score: None,
        priority: 60,
        dedupe_key: Some(id),
        budget_impact_tokens: None,
        selection_reason: "hook output is injected directly as system context".to_string(),
        availability_reason: "available from hook runtime output buffer".to_string(),
        not_selected_reason: "already injected as direct system context".to_string(),
        selectable: false,
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
    message.role == "system" && self::compact::compact_boundary_item(&message.content).is_some()
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

fn is_interrupt_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::Interrupted)
            || cause
                .to_string()
                .to_ascii_lowercase()
                .contains("cancelled by user")
    })
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
