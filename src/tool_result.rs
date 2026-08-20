mod batch;
mod content;
mod projection;
mod render;
mod store;
mod transcript;

#[cfg(test)]
use batch::TOOL_RESULT_BATCH_BUDGET;
pub use batch::enforce_tool_result_batch_budget;
#[cfg(test)]
use content::tool_result_content_candidates;
pub use projection::{
    MICROCOMPACT_CLEARED_MESSAGE, ToolResultProjectionPolicy, ToolResultProjectionReport,
    project_tool_results_for_context,
};
#[cfg(test)]
use render::{compact_read_file, compact_subagent_result, compact_web_search};
pub(crate) use render::{model_preview_bash_output, render_bash_outcome_summary};
pub use store::{ToolResultStore, default_tool_result_store_dir};
pub use transcript::repair_tool_result_history;

#[cfg(test)]
#[path = "tool_result/tests.rs"]
mod tests;
