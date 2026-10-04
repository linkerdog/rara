use rara_memory::memory_handle::MemoryHandle;
use rara_persistence::thread_turn_log;
use rara_provider_catalog::ModelCatalogEntry;
use rara_state::state_db::{
    PersistedCompactState, PersistedPromptRuntimeState, PersistedStructuredRolloutEvent, StateDb,
};
use rara_tools::tool::ToolManager;
use tempfile::tempdir;

use super::{
    ActivePendingInteractionKind, AgentMarkdownStreamState, InteractionKind, ListPickerKind,
    ModelCatalogSnapshot, Overlay, PROVIDER_FAMILIES, PendingInteractionSnapshot, ProviderFamily,
    RuntimeExtensionSnapshot, RuntimeSnapshot, SystemMessageKind, ToolTranscriptStatus,
    TranscriptEntry, TranscriptScrollLayout, TranscriptTurn, TuiApp,
    input_requests_command_palette, parse_repo_slug, state_db_status_error,
};
use crate::agent::{Agent, PendingApproval};
use crate::codex_model_catalog::{CodexModelOption, CodexReasoningOption};
use crate::config::{ConfigManager, OpenAiEndpointKind, RaraConfig};
use crate::config::{DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_MODEL};
use crate::llm::MockLlm;
use crate::session::SessionManager;
use crate::tools::agent::{AgentDefinitionCache, AgentDefinitionLoadRecord};
use crate::tools::bash::BashCommandInput;
use crate::tui::command::palette_commands;
use crate::workspace::WorkspaceMemory;

fn provider_family_idx(family: ProviderFamily) -> usize {
    PROVIDER_FAMILIES
        .iter()
        .position(|(candidate, _, _)| *candidate == family)
        .expect("provider family present")
}

mod core_and_runtime;
mod providers;
mod transcript_and_persistence;
