use crate::agent::{CompactState, PlanStepStatus};
use crate::prompt::{EffectivePrompt, PromptSource};
use crate::todo::{TodoState, TodoSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    /// Content was served from the in-memory cache (mtime unchanged).
    Hit,
    /// Content was re-read from disk (mtime changed or first read).
    Miss,
    /// Cache does not apply (non-file source, e.g. append-system-prompt).
    NoCache,
}

pub const RETRIEVED_WORKSPACE_MEMORY_KIND: &str = "retrieved_workspace_memory";
pub const RETRIEVED_THREAD_CONTEXT_KIND: &str = "retrieved_thread_context";

pub fn is_retrieved_memory_kind(kind: &str) -> bool {
    matches!(
        kind,
        RETRIEVED_WORKSPACE_MEMORY_KIND | RETRIEVED_THREAD_CONTEXT_KIND
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextAssemblyEntry {
    pub order: usize,
    pub cache_status: Option<CacheStatus>,
    pub layer: String,
    pub kind: String,
    pub label: String,
    pub source_path: Option<String>,
    pub injected: bool,
    pub inclusion_reason: String,
    pub budget_impact_tokens: Option<usize>,
    pub dropped_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextAssemblyView {
    pub entries: Vec<ContextAssemblyEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSourceContextEntry {
    pub order: usize,
    pub kind: String,
    pub label: String,
    pub display_path: String,
    pub status_line: String,
    pub inclusion_reason: String,
}

impl PromptSourceContextEntry {
    fn from_prompt_source(order: usize, source: &PromptSource) -> Self {
        Self {
            order,
            kind: source.kind_label().to_string(),
            label: source.label.clone(),
            display_path: source.display_path.clone(),
            status_line: source.status_line(),
            inclusion_reason: source.inclusion_reason().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContextView {
    pub base_prompt_kind: String,
    pub section_keys: Vec<String>,
    pub source_entries: Vec<PromptSourceContextEntry>,
    pub source_status_lines: Vec<String>,
    pub append_system_prompt: Option<String>,
    pub warnings: Vec<String>,
}

impl PromptContextView {
    pub fn from_effective_prompt(
        effective_prompt: EffectivePrompt,
        append_system_prompt: Option<String>,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            base_prompt_kind: effective_prompt.base_prompt_kind.label().to_string(),
            section_keys: effective_prompt
                .section_keys
                .iter()
                .map(|key| (*key).to_string())
                .collect(),
            source_entries: effective_prompt
                .sources
                .iter()
                .enumerate()
                .map(|(idx, source)| PromptSourceContextEntry::from_prompt_source(idx + 1, source))
                .collect(),
            source_status_lines: effective_prompt
                .sources
                .iter()
                .map(|source| source.status_line())
                .collect(),
            append_system_prompt,
            warnings,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanContextView {
    pub execution_mode: String,
    pub steps: Vec<(String, String)>,
    pub explanation: Option<String>,
}

impl PlanContextView {
    pub fn from_agent_state(
        execution_mode: &str,
        steps: impl Iterator<Item = (PlanStepStatus, String)>,
        explanation: Option<String>,
    ) -> Self {
        Self {
            execution_mode: execution_mode.to_string(),
            steps: steps
                .map(|(status, step)| {
                    let status = match status {
                        PlanStepStatus::Pending => "pending",
                        PlanStepStatus::InProgress => "in_progress",
                        PlanStepStatus::Completed => "completed",
                    };
                    (status.to_string(), step)
                })
                .collect(),
            explanation,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoContextView {
    pub summary: TodoSummary,
    pub updated_at: Option<i64>,
    pub items: Vec<(String, String, String)>,
}

impl TodoContextView {
    pub fn from_state(state: Option<TodoState>) -> Self {
        match state {
            Some(state) => Self {
                summary: state.summary(),
                updated_at: Some(state.updated_at),
                items: state
                    .items
                    .into_iter()
                    .map(|item| (item.id, item.status.as_str().to_string(), item.content))
                    .collect(),
            },
            None => Self {
                summary: TodoSummary::default(),
                updated_at: None,
                items: Vec::new(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetView {
    pub context_window_tokens: Option<usize>,
    pub compact_threshold_tokens: usize,
    pub reserved_output_tokens: usize,
    pub stable_instructions_budget: usize,
    pub workspace_prompt_budget: usize,
    pub active_turn_budget: usize,
    pub compacted_history_budget: usize,
    pub retrieved_memory_budget: usize,
    pub remaining_input_budget: Option<usize>,
}

impl ContextBudgetView {
    pub fn from_compact_state(
        state: &CompactState,
        system_prompt_budget: usize,
        stable_instructions_budget: usize,
        workspace_prompt_budget: usize,
        active_turn_budget: usize,
        compacted_history_budget: usize,
        retrieved_memory_budget: usize,
    ) -> Self {
        let remaining_input_budget = state.context_window_tokens.map(|window| {
            window
                .saturating_sub(state.reserved_output_tokens)
                .saturating_sub(system_prompt_budget)
                .saturating_sub(active_turn_budget)
                .saturating_sub(compacted_history_budget)
                .saturating_sub(retrieved_memory_budget)
        });
        Self {
            context_window_tokens: state.context_window_tokens,
            compact_threshold_tokens: state.compact_threshold_tokens,
            reserved_output_tokens: state.reserved_output_tokens,
            stable_instructions_budget,
            workspace_prompt_budget,
            active_turn_budget,
            compacted_history_budget,
            retrieved_memory_budget,
            remaining_input_budget,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionContextView {
    pub estimated_history_tokens: usize,
    pub context_window_tokens: Option<usize>,
    pub compact_threshold_tokens: usize,
    pub reserved_output_tokens: usize,
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_files: Vec<String>,
    pub last_compaction_boundary_version: Option<u32>,
    pub last_compaction_boundary_before_tokens: Option<usize>,
    pub last_compaction_boundary_recent_file_count: Option<usize>,
    pub source_entries: Vec<CompactionSourceContextEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionSourceContextEntry {
    pub order: usize,
    pub kind: String,
    pub label: String,
    pub source_descriptor: String,
    pub detail: String,
    pub inclusion_reason: String,
}

impl CompactionContextView {
    pub fn from_compact_state(state: &CompactState) -> Self {
        Self {
            estimated_history_tokens: state.estimated_history_tokens,
            context_window_tokens: state.context_window_tokens,
            compact_threshold_tokens: state.compact_threshold_tokens,
            reserved_output_tokens: state.reserved_output_tokens,
            compaction_count: state.compaction_count,
            last_compaction_before_tokens: state.last_compaction_before_tokens,
            last_compaction_after_tokens: state.last_compaction_after_tokens,
            last_compaction_recent_files: state.last_compaction_recent_files.clone(),
            last_compaction_boundary_version: state
                .last_compaction_boundary
                .map(|boundary| boundary.version),
            last_compaction_boundary_before_tokens: state
                .last_compaction_boundary
                .map(|boundary| boundary.before_tokens),
            last_compaction_boundary_recent_file_count: state
                .last_compaction_boundary
                .map(|boundary| boundary.recent_file_count),
            source_entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedRuntimeContext {
    pub cwd: String,
    pub branch: String,
    pub session_id: String,
    pub history_len: usize,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub total_cache_hit_tokens: u32,
    pub total_cache_miss_tokens: u32,
    pub budget: ContextBudgetView,
    pub assembly: ContextAssemblyView,
    pub prompt: PromptContextView,
    pub plan: PlanContextView,
    pub todo: TodoContextView,
    pub compaction: CompactionContextView,
    pub retrieval: RetrievalContextView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalContextView {
    pub entries: Vec<RetrievalSourceContextEntry>,
    pub orchestration: RetrievalOrchestrationView,
    pub memory_selection: MemorySelectionContextView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalSourceContextEntry {
    pub order: usize,
    pub kind: String,
    pub label: String,
    pub status: String,
    pub detail: String,
    pub inclusion_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievedMemoryCandidate {
    pub kind: String,
    pub label: String,
    pub detail: String,
    pub selection_reason: String,
    pub rank: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalSourceRef {
    pub source_type: String,
    pub source_id: Option<String>,
    pub source_path: Option<String>,
    pub source_uri: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalCandidate {
    pub id: String,
    pub source: RetrievalSourceRef,
    pub kind: String,
    pub scope: String,
    pub label: String,
    pub detail: String,
    pub summary: Option<String>,
    pub rank: usize,
    pub score: Option<f32>,
    pub priority: usize,
    pub dedupe_key: Option<String>,
    pub budget_impact_tokens: Option<usize>,
    pub selection_reason: String,
    pub availability_reason: String,
    pub not_selected_reason: String,
    pub selectable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct RetrievalOrchestrationView {
    pub request_id: String,
    pub query: String,
    pub providers: Vec<RetrievalProviderStatus>,
    pub candidates: Vec<RetrievalCandidateContextEntry>,
    pub selected: Vec<RetrievalCandidateContextEntry>,
    pub available: Vec<RetrievalCandidateContextEntry>,
    pub dropped: Vec<RetrievalCandidateContextEntry>,
    pub budget: RetrievalBudgetContextView,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetrievalProviderStatus {
    pub order: usize,
    pub kind: String,
    pub label: String,
    pub status: String,
    pub detail: String,
    pub inclusion_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetrievalCandidateContextEntry {
    pub order: usize,
    pub kind: String,
    pub label: String,
    pub detail: String,
    pub status: String,
    pub source_kind: String,
    pub budget_impact_tokens: Option<usize>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct RetrievalBudgetContextView {
    pub selection_budget_tokens: Option<usize>,
    pub selected_tokens: usize,
    pub available_tokens: usize,
    pub dropped_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemorySelectionContextView {
    pub selection_budget_tokens: Option<usize>,
    pub selected_items: Vec<MemorySelectionItemContextEntry>,
    pub available_items: Vec<MemorySelectionItemContextEntry>,
    pub dropped_items: Vec<MemorySelectionItemContextEntry>,
}

/// Reason an item was not selected into the current turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason {
    /// Did not win the selection contest (ranking, dedup, not selectable,
    /// compacted history already covers it). Appears in available_items.
    NotSelected { reason: String },
    /// Would have exceeded the remaining memory-selection budget.
    /// Appears in dropped_items.
    BudgetExceeded { reason: String },
}

impl DropReason {
    pub fn reason(&self) -> &str {
        match self {
            DropReason::NotSelected { reason } | DropReason::BudgetExceeded { reason } => reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySelectionItemContextEntry {
    pub order: usize,
    pub kind: String,
    pub label: String,
    pub detail: String,
    pub selection_reason: String,
    pub budget_impact_tokens: Option<usize>,
    pub dropped_reason: Option<DropReason>,
}
