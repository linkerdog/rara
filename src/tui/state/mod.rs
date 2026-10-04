use bottom_pane_model::BottomPaneModel;
mod approval_details;
pub(crate) use approval_details::{ApprovalDetailNavigation, ApprovalDetailScroll};
mod bottom_pane_model;
mod notices;
pub(crate) use notices::NoticeLevel;
mod overlay_state;
mod pending_interaction;
mod persistence;
mod planning_lifecycle;
mod quit_shortcut;
pub(crate) use quit_shortcut::{QuitShortcutAction, QuitShortcutKey, QuitShortcutState};
mod runtime_snapshot;
mod scroll_acceleration;
pub(crate) use scroll_acceleration::ScrollAcceleration;
mod shared_tasks;
mod state_presets;
#[cfg(test)]
mod tests;
mod transcript;
mod transcript_scroll;
mod types;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rara_persistence::redaction::redact_secrets;
use rara_provider_catalog::ModelCatalogEntry;
use rara_provider_catalog::{ModelCatalogProvider, fallback_models};
use rara_state::state_db::StateDb;

pub use self::planning_lifecycle::{
    PlanningApprovalDecision, PlanningApprovalStatus, PlanningLifecycleSnapshot,
};
pub use self::state_presets::{
    current_model_presets, openai_compatible_preset_kind, selected_preset_idx_for_config,
    selected_provider_family_idx_for_config,
};
pub(crate) use self::transcript_scroll::{TranscriptScroll, TranscriptScrollLayout};
use self::types::CommittedTranscriptRenderCache;
#[cfg(test)]
pub use self::types::current_unix_timestamp_secs;
pub use self::types::{
    ActiveLiveSections, ActivePendingInteraction, ActivePendingInteractionKind,
    AgentMarkdownStreamState, ApiKeyTarget, CommandSpec, CompactionTranscriptPayload,
    CompletedInteractionSnapshot, GoalHandle, GoalStatus, HelpTab, InteractionKind, ListPickerKind,
    LocalCommand, LocalCommandKind, ModelCatalogSnapshot, ModelRoutingView, OAuthLoginMode,
    OpenAiModelPickerAction, Overlay, PROVIDER_FAMILIES, PendingApprovalSnapshot,
    PendingInteractionSnapshot, PermissionMode, ProviderFamily, RalphGoal, RunningTask,
    RuntimeExtensionSnapshot, RuntimePhase, RuntimeSnapshot, SkillPickerEntry, StatusTab,
    SystemMessageKind, TaskCompletion, TaskKind, TerminalDiagnosticsView, ToolTranscriptPayload,
    ToolTranscriptStatus, TranscriptEntry, TranscriptEntryPayload, TranscriptTurn, TuiApp,
    TuiEvent, UnifiedModelPreset,
};
use super::queued_input::PendingFollowUpMessage;
use crate::agent::{AgentExecutionMode, BashApprovalMode};
use crate::codex_model_catalog::{CodexModelOption, CodexReasoningOption};
use crate::config::{ConfigManager, DEFAULT_CODEX_BASE_URL, OpenAiEndpointKind};
use crate::oauth::OAuthManager;
pub(crate) use crate::runtime_client::RebuildSuccess;

mod composer;
mod initialization;
mod model_catalog;
mod model_selection;
mod provider_setup;
mod support;

pub(super) use support::INPUT_HISTORY_LIMIT;
use support::{
    TextInputTarget, effective_cursor_offset, startup_warning_for_config, state_db_status_error,
    terminal_multiplexer_label, terminal_remote_label,
};
pub(crate) use support::{char_offset_to_byte_index, contains_structured_planning_output};
pub use support::{input_requests_command_palette, openai_profile_setup_kinds};

mod helpers;
pub(crate) use helpers::*;
mod permissions;
