use std::sync::OnceLock;
use std::time::Duration;

use crate::context::RetrievedMemoryCandidate;
use crate::llm::{ContextBudget, is_context_window_error};
use crate::session::PersistedCompactionEvent;

pub(crate) const RECENT_FILE_CARRY_OVER_LIMIT: usize = 5;
pub(crate) const RECENT_FILE_EXCERPT_LIMIT: usize = 3;
pub(crate) const RECENT_FILE_EXCERPT_CHAR_LIMIT: usize = 600;
pub(crate) const MEMORY_CARRY_OVER_LIMIT: usize = 3;
pub(crate) const SKILL_CARRY_OVER_LIMIT: usize = 3;
pub(crate) const SKILL_INSTRUCTION_PREVIEW_CHAR_LIMIT: usize = 600;
pub(crate) const HOOK_CARRY_OVER_LIMIT: usize = 3;
pub(crate) const MCP_CARRY_OVER_LIMIT: usize = 3;
pub(crate) const RETAINED_HISTORY_BUDGET_FRACTION: usize = 2;
pub(crate) const COMPACT_BOUNDARY_KIND: &str = "compact_boundary";
pub(crate) const COMPACT_BOUNDARY_VERSION: u32 = 1;
pub(crate) const ROLE_ASSISTANT: &str = "assistant";
// Wait for about two 4K chunks of new context before retrying automatic
// compaction after a timeout or backend failure.
pub(crate) const AUTO_COMPACTION_RETRY_HYSTERESIS_TOKENS: usize = 8_192;
#[cfg(not(test))]
pub(crate) const COMPACTION_SUMMARY_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(test)]
pub(crate) const TEST_COMPACTION_SUMMARY_TIMEOUT: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompactBoundaryMetadata {
    pub version: u32,
    pub before_tokens: usize,
    pub recent_file_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentFileExcerpt {
    pub(crate) path: String,
    pub(crate) line_range: Option<(usize, usize)>,
    pub(crate) snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiRoundGroup {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) token_estimate: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactPlan {
    pub(crate) summarize_end: usize,
    pub(crate) retained_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactCarryOver {
    pub(crate) summary: String,
    pub(crate) recent_files: Vec<String>,
    pub(crate) recent_file_excerpts: Vec<RecentFileExcerpt>,
    pub(crate) retrieved_memory: Vec<RetrievedMemoryCandidate>,
    pub(crate) invoked_skills: Vec<InvokedSkillCarryOver>,
    pub(crate) retained_hooks: Vec<RetainedContextCarryOver>,
    pub(crate) retained_mcp: Vec<RetainedContextCarryOver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvokedSkillCarryOver {
    pub(crate) name: String,
    pub(crate) title: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) display_path: Option<String>,
    pub(crate) args: Option<String>,
    pub(crate) instruction_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetainedContextCarryOver {
    pub(crate) label: String,
    pub(crate) source_descriptor: String,
    pub(crate) detail: String,
    pub(crate) inclusion_reason: Option<String>,
}

#[derive(Debug)]
pub(crate) struct CompactionSummaryTimeout;

impl std::fmt::Display for CompactionSummaryTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "history compaction timed out after {} seconds",
            COMPACTION_SUMMARY_TIMEOUT.as_secs()
        )
    }
}

impl std::error::Error for CompactionSummaryTimeout {}

#[derive(Debug, Clone, Default)]
pub struct CompactState {
    pub estimated_history_tokens: usize,
    pub context_window_tokens: Option<usize>,
    pub compact_threshold_tokens: usize,
    pub reserved_output_tokens: usize,
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_files: Vec<String>,
    pub last_compaction_boundary: Option<CompactBoundaryMetadata>,
    pub consecutive_auto_compaction_failures: usize,
    pub auto_compaction_retry_after_tokens: Option<usize>,
}

