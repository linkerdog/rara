mod compact;
mod context_view;
mod control_handler;
mod execution;
mod memory_retrieval;
mod planning;
mod prompting;
mod runtime;
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
    agent_tree_control: Option<Arc<crate::tools::agent::AgentTreeControl>>,
    cancellation_token: Option<Arc<AtomicBool>>,
    last_interaction_time: std::time::Instant,
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
