use rara_tools::tool::ToolOutputStream;
use serde_json::json;
use tempfile::tempdir;

use super::delegated_result::delegated_result;
use super::helpers::{
    format_apply_patch_result, format_apply_patch_use, format_tool_result, format_tool_use,
    is_oauth_prompt_message, planning_note_lines, scrub_internal_control_tokens,
};
use super::{apply_tui_event, format_memory_event_notice, runtime_event_from_agent_event};
use crate::agent::{AgentEvent, AgentExecutionMode};
use crate::config::ConfigManager;
use crate::runtime_control::{MemoryEvent, MemoryRecordSummary, RuntimeEvent, RuntimeProvenance};
use crate::session_promotion::{
    SessionShardPromotionDecision, SessionShardPromotionOutcome, SessionShardPromotionPlan,
    SessionShardPromotionSkipReason, SessionShardPromotionTrigger,
};
use crate::tui::state::{ActivePendingInteractionKind, TranscriptEntryPayload};
use crate::tui::state::{RuntimePhase, TuiApp, TuiEvent};
use crate::tui::terminal_event::{TerminalEvent, TerminalTarget};
use crate::tui::tool_progress::format_tool_progress;

#[path = "tests/cases_1.rs"]
mod cases_1;
#[path = "tests/cases_2.rs"]
mod cases_2;
#[path = "tests/patch_preview.rs"]
mod patch_preview;
