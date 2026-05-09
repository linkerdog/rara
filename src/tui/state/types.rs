use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Instant;

use rara_state::state_db::StateDb;
use ratatui::text::Line;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

use super::super::markdown_stream::MarkdownStreamCollector;
use super::super::queued_input::PendingFollowUpMessage;
use crate::agent::{Agent, AgentExecutionMode, BashApprovalMode};
use crate::codex_model_catalog::CodexModelOption;
use crate::config::{ConfigManager, OpenAiEndpointKind, RaraConfig};
use crate::context::{
    CompactionSourceContextEntry, ContextAssemblyEntry, PromptSourceContextEntry,
    RetrievalSourceContextEntry,
};
use crate::control_tokens::{has_pending_internal_control_context, scrub_internal_control_tokens};
use crate::mcp_tool_cache::McpToolCache;
use crate::oauth::SavedCodexAuthMode;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::thread_store::ThreadSummary;
use crate::tool::ToolOutputStream;
use crate::tools::bash::BashCommandInput;
use crate::tui::display_sanitize::sanitize_display_text;
use crate::tui::terminal_event::TerminalEvent;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HelpTab {
    General,
    Commands,
    Runtime,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusTab {
    Overview,
    Config,
    Context,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overlay {
    Help(HelpTab),
    CommandPalette,
    Status(StatusTab),
    Context,
    BaseUrlEditor,
    ApiKeyEditor,
    ModelNameEditor,
    OpenAiProfileLabelEditor,
    SkillsPicker,
    /// Generic list-picker overlay — render content driven by ListPickerKind.
    ListPicker(ListPickerKind),
    PermissionPicker,
}

/// Identifies which content a generic `Overlay::ListPicker` should render.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListPickerKind {
    Provider,
    Model,
    OpenAiEndpointKind,
    OpenAiProfile,
    Resume,
    AuthMode,
    ReasoningEffort,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProviderFamily {
    Codex,
    DeepSeek,
    OpenAiCompatible,
    Gemini,
    CandleLocal,
    Ollama,
    Bedrock,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenAiModelPickerAction {
    SelectProfile,
    DeleteProfile,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PermissionMode {
    Auto,
    ReadOnly,
    AcceptEdits,
    FullAccess,
    Custom,
}

impl PermissionMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Auto => Self::AcceptEdits,
            Self::AcceptEdits => Self::ReadOnly,
            Self::ReadOnly => Self::FullAccess,
            Self::Custom => Self::Auto,
            Self::FullAccess => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AcceptEdits => "accept-edits",
            Self::ReadOnly => "read-only",
            Self::Custom => "custom",
            Self::FullAccess => "full-access",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LocalCommandKind {
    Help,
    Status,
    Context,
    Clear,
    Resume,
    Plan,
    Approval,
    Compact,
    Model,
    Connect,
    BaseUrl,
    Login,
    Mcp,
    Permissions,
    Logout,
    Review,
    Goal,
    Quit,
    Skills,
}

pub struct LocalCommand {
    pub kind: LocalCommandKind,
    pub arg: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRoutingView {
    pub main_model: String,
    pub main_source: String,
    pub auxiliary_model: String,
    pub auxiliary_source: String,
    pub auxiliary_route: String,
    pub auxiliary_uses_main_model: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalDiagnosticsView {
    pub name: String,
    pub user_agent: String,
    pub term_program: Option<String>,
    pub term: Option<String>,
    pub multiplexer: String,
    pub remote: String,
    pub history_mode: String,
    pub focused: bool,
    pub width_columns: u16,
}

pub struct CommandSpec {
    pub category: &'static str,
    pub name: &'static str,
    pub usage: &'static str,
    pub summary: &'static str,
    pub detail: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuntimePhase {
    Idle,
    LocalCommand,
    SendingPrompt,
    ProcessingResponse,
    RunningTool,
    RebuildingBackend,
    BackendReady,
    OAuthStarting,
    OAuthWaitingCallback,
    OAuthExchangingToken,
    OAuthDeviceCodePrompt,
    OAuthPollingDeviceCode,
    OAuthSaved,
    Failed,
}

impl Default for RuntimePhase {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Default, Clone)]
pub struct RuntimeSnapshot {
    pub cwd: String,
    pub branch: String,
    pub session_id: String,
    pub history_len: usize,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub total_cache_hit_tokens: u32,
    pub total_cache_miss_tokens: u32,
    pub estimated_history_tokens: usize,
    pub context_window_tokens: Option<usize>,
    pub compact_threshold_tokens: usize,
    pub reserved_output_tokens: usize,
    pub stable_instructions_budget: usize,
    pub workspace_prompt_budget: usize,
    pub active_turn_budget: usize,
    pub compacted_history_budget: usize,
    pub retrieved_memory_budget: usize,
    pub remaining_input_budget: Option<usize>,
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_files: Vec<String>,
    pub last_compaction_boundary_version: Option<u32>,
    pub last_compaction_boundary_before_tokens: Option<usize>,
    pub last_compaction_boundary_recent_file_count: Option<usize>,
    pub compaction_source_entries: Vec<CompactionSourceContextEntry>,
    pub plan_steps: Vec<(String, String)>,
    pub plan_explanation: Option<String>,
    pub pending_interactions: Vec<PendingInteractionSnapshot>,
    pub completed_interactions: Vec<CompletedInteractionSnapshot>,
    pub todo: crate::context::TodoContextView,
    pub todo_artifact_path: Option<String>,
    pub prompt_base_kind: String,
    pub prompt_section_keys: Vec<String>,
    pub prompt_source_entries: Vec<PromptSourceContextEntry>,
    pub prompt_source_status_lines: Vec<String>,
    pub prompt_append_system_prompt: Option<String>,
    pub prompt_warnings: Vec<String>,
    pub retrieval_source_entries: Vec<RetrievalSourceContextEntry>,
    pub retrieval_orchestration: crate::context::RetrievalOrchestrationView,
    pub memory_selection: crate::context::MemorySelectionContextView,
    pub context_observability: crate::context::ContextObservabilityView,
    pub assembly_entries: Vec<ContextAssemblyEntry>,
    // ── Extensions ──────────────────────────────────────
    pub extension_skill_count: usize,
    pub extension_skill_scopes: Vec<String>,
    pub extension_hook_count: usize,
    pub extension_agent_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionKind {
    RequestInput,
    Approval,
    PlanApproval,
}

#[derive(Default, Clone)]
pub struct PendingApprovalSnapshot {
    pub tool_use_id: String,
    pub command: String,
    pub allow_net: bool,
    pub payload: BashCommandInput,
}

#[derive(Clone)]
pub struct PendingInteractionSnapshot {
    pub kind: InteractionKind,
    pub title: String,
    pub summary: String,
    pub options: Vec<(String, String)>,
    pub note: Option<String>,
    pub approval: Option<PendingApprovalSnapshot>,
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivePendingInteractionKind {
    PlanApproval,
    ShellApproval,
    PlanningQuestion,
    ExplorationQuestion,
    SubAgentQuestion,
    RequestInput,
}

pub struct ActivePendingInteraction<'a> {
    pub kind: ActivePendingInteractionKind,
    pub _snapshot: &'a PendingInteractionSnapshot,
}

#[derive(Clone)]
pub struct CompletedInteractionSnapshot {
    pub kind: InteractionKind,
    pub title: String,
    pub summary: String,
    pub source: Option<String>,
}

pub enum TaskKind {
    Query,
    Compact,
    Rebuild,
    OAuth,
    GoogleOAuth,
    DeepSeekModels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthLoginMode {
    Browser,
    DeviceCode,
}

pub enum TaskCompletion {
    Query {
        agent: Agent,
        result: anyhow::Result<()>,
    },
    Compact {
        agent: Agent,
        result: anyhow::Result<bool>,
    },
    Rebuild {
        result: anyhow::Result<RebuildSuccess>,
    },
    OAuth {
        mode: OAuthLoginMode,
        result: anyhow::Result<secrecy::SecretString>,
    },
    GoogleOAuth {
        mode: OAuthLoginMode,
        result: anyhow::Result<crate::google_oauth::GoogleCredential>,
    },
    DeepSeekModels {
        result: anyhow::Result<Vec<String>>,
    },
}

pub struct RebuildSuccess {
    pub agent: Agent,
    pub warnings: Vec<String>,
    pub sandbox_network_access: Arc<AtomicBool>,
    /// Shared goal handle for model-facing tools.
    pub goal_handle: crate::tui::state::GoalHandle,
    pub mcp_tool_cache: crate::mcp_tool_cache::McpToolCache,
}

pub enum TuiEvent {
    Transcript {
        role: &'static str,
        message: String,
    },
    Terminal(TerminalEvent),
    ToolProgress {
        name: String,
        stream: ToolOutputStream,
        chunk: String,
    },
}

pub struct RunningTask {
    pub kind: TaskKind,
    pub receiver: UnboundedReceiver<TuiEvent>,
    pub handle: JoinHandle<TaskCompletion>,
    pub started_at: Instant,
    pub next_heartbeat_after_secs: u64,
    pub cancellation_token: Option<Arc<AtomicBool>>,
    pub cancellation_requested: bool,
}

pub const PROVIDER_FAMILIES: [(ProviderFamily, &str, &str); 7] = [
    (
        ProviderFamily::Codex,
        "Codex",
        "Use Codex with browser login, device-code login, or a Codex API key.",
    ),
    (
        ProviderFamily::DeepSeek,
        "DeepSeek",
        "Use DeepSeek with an API key and model list from api.deepseek.com.",
    ),
    (
        ProviderFamily::OpenAiCompatible,
        "OpenAI-compatible",
        "Use OpenAI-compatible endpoint profiles such as Custom, Kimi, or OpenRouter.",
    ),
    (
        ProviderFamily::Gemini,
        "Gemini",
        "Use Google Gemini via AI Studio (API key) or Code Assist (OAuth).",
    ),
    (
        ProviderFamily::CandleLocal,
        "Candle Local",
        "Run local Candle models directly in-process.",
    ),
    (
        ProviderFamily::Ollama,
        "Ollama",
        "Use an external Ollama server and choose a local tag.",
    ),
    (
        ProviderFamily::Bedrock,
        "Bedrock",
        "Use AWS Bedrock with the Converse API. Credentials from the default AWS chain.",
    ),
];

#[derive(Clone, Default)]
pub struct TranscriptEntry {
    pub role: String,
    pub message: String,
    pub payload: Option<TranscriptEntryPayload>,
}

#[derive(Clone, Debug)]
pub enum TranscriptEntryPayload {
    Terminal(TerminalEvent),
}

impl TranscriptEntry {
    pub fn new(role: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            message: message.into(),
            payload: None,
        }
    }

    pub fn terminal_event(event: TerminalEvent) -> Self {
        Self {
            role: "Terminal Event".to_string(),
            message: event.to_transcript_message(),
            payload: Some(TranscriptEntryPayload::Terminal(event)),
        }
    }
}

#[derive(Clone, Default)]
pub struct TranscriptTurn {
    pub entries: Vec<TranscriptEntry>,
}

#[derive(Default)]
pub(crate) struct CommittedTranscriptRenderCache {
    pub generation: u64,
    pub width: u16,
    pub lines: Vec<Line<'static>>,
}

pub struct AgentMarkdownStreamState {
    pub(crate) raw_text: String,
    last_visible_text: String,
    incremental_passthrough: bool,
    cwd: PathBuf,
    collector: MarkdownStreamCollector,
    committed_lines: Vec<Line<'static>>,
    pub(crate) display_lines: Vec<Line<'static>>,
}

impl AgentMarkdownStreamState {
    pub(crate) fn new(cwd: PathBuf) -> Self {
        Self {
            raw_text: String::new(),
            last_visible_text: String::new(),
            incremental_passthrough: true,
            cwd: cwd.clone(),
            collector: MarkdownStreamCollector::new(None, &cwd),
            committed_lines: Vec::new(),
            display_lines: Vec::new(),
        }
    }

    pub(crate) fn push_delta(&mut self, delta: &str) {
        if self.incremental_passthrough && !delta.contains('<') {
            self.raw_text.push_str(delta);
            let visible_delta = sanitize_display_text(delta);
            self.last_visible_text.push_str(&visible_delta);
            if !visible_delta.is_empty() {
                self.collector.push_delta(&visible_delta);
                self.refresh_display_lines();
            }
            return;
        }

        self.raw_text.push_str(delta);
        let visible_text = sanitize_display_text(&scrub_internal_control_tokens(&self.raw_text));
        if let Some(new_visible_delta) = visible_text.strip_prefix(&self.last_visible_text) {
            if !new_visible_delta.is_empty() {
                self.collector.push_delta(new_visible_delta);
                self.refresh_display_lines();
            }
        } else {
            self.replace_display_text(&visible_text);
        }
        self.last_visible_text = visible_text;
        self.incremental_passthrough = !has_pending_internal_control_context(&self.raw_text);
    }

    pub(crate) fn sanitized_raw_text(&self) -> String {
        sanitize_display_text(&scrub_internal_control_tokens(&self.raw_text))
    }

    fn replace_display_text(&mut self, text: &str) {
        self.collector = MarkdownStreamCollector::new(None, &self.cwd);
        self.committed_lines.clear();
        self.display_lines.clear();
        self.collector.push_delta(text);
        self.refresh_display_lines();
    }

    fn refresh_display_lines(&mut self) {
        self.committed_lines
            .extend(self.collector.commit_complete_lines());
        self.display_lines = self.committed_lines.clone();
        self.display_lines.extend(self.collector.preview_lines());
    }
}

#[derive(Clone)]
pub enum ActiveLiveEvent {
    Thinking(String),
    ExplorationAction(String),
    ExplorationNote(String),
    PlanningAction(String),
    PlanningNote(String),
    RunningAction(String),
}

impl ActiveLiveEvent {
    pub fn role(&self) -> &'static str {
        match self {
            Self::Thinking(_) => "Thinking",
            Self::ExplorationAction(_) | Self::ExplorationNote(_) => "Exploring",
            Self::PlanningAction(_) | Self::PlanningNote(_) => "Planning",
            Self::RunningAction(_) => "Running",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Thinking(message)
            | Self::ExplorationAction(message)
            | Self::ExplorationNote(message)
            | Self::PlanningAction(message)
            | Self::PlanningNote(message)
            | Self::RunningAction(message) => message,
        }
    }

    pub fn is_note(&self) -> bool {
        matches!(self, Self::ExplorationNote(_) | Self::PlanningNote(_))
    }
}

#[derive(Default)]
pub struct ActiveLiveSections {
    pub events: Vec<ActiveLiveEvent>,
    pub exploration_actions: Vec<String>,
    pub exploration_notes: Vec<String>,
    pub planning_actions: Vec<String>,
    pub planning_notes: Vec<String>,
    pub running_actions: Vec<String>,
}

pub struct TuiApp {
    pub input: String,
    pub input_cursor_offset: Option<usize>,
    pub input_history: Vec<String>,
    pub input_history_cursor: Option<usize>,
    pub input_history_draft: Option<String>,
    pub committed_turns: Vec<TranscriptTurn>,
    pub active_turn: TranscriptTurn,
    pub overlay: Option<Overlay>,
    /// Dialog stack for back-navigation. The last element is always the
    /// current overlay.  When empty, no overlay is shown.
    pub overlay_stack: Vec<Overlay>,
    /// Whether the sidebar is visible in wide-screen mode.
    /// Toggled with Ctrl+B.
    pub sidebar_visible: bool,
    pub config: RaraConfig,
    pub config_manager: ConfigManager,
    pub setup_status: Option<String>,
    pub notice: Option<String>,
    pub runtime_phase: RuntimePhase,
    pub runtime_phase_detail: Option<String>,
    pub snapshot: RuntimeSnapshot,
    pub agent_execution_mode: AgentExecutionMode,
    pub bash_approval_mode: BashApprovalMode,
    pub provider_picker_idx: usize,
    pub model_picker_idx: usize,
    pub openai_endpoint_kind_picker_idx: usize,
    pub openai_profile_picker_idx: usize,
    pub reasoning_effort_picker_idx: usize,
    pub auth_mode_idx: usize,
    pub permission_picker_idx: usize,
    pub command_palette_idx: usize,
    pub base_url_input: String,
    pub base_url_cursor_offset: Option<usize>,
    pub api_key_input: String,
    pub api_key_cursor_offset: Option<usize>,
    pub model_name_input: String,
    pub model_name_cursor_offset: Option<usize>,
    pub openai_profile_label_input: String,
    pub openai_profile_label_cursor_offset: Option<usize>,
    pub openai_profile_label_kind: Option<OpenAiEndpointKind>,
    pub openai_setup_steps: Vec<Overlay>,
    pub openai_setup_keep_empty_api_key: bool,
    pub codex_model_options: Vec<CodexModelOption>,
    pub deepseek_model_options: Vec<String>,
    pub recent_commands: Vec<String>,
    pub recent_threads: Vec<ThreadSummary>,
    pub resume_picker_idx: usize,
    pub committed_render_generation: u64,
    pub committed_render_cache: RefCell<CommittedTranscriptRenderCache>,
    pub transcript_scroll: usize,
    /// Scroll offset for multiline composer content (lines from top).
    pub composer_scroll: usize,
    pub context_scroll: u16,
    pub terminal_width: u16,
    pub agent_markdown_stream: Option<AgentMarkdownStreamState>,
    pub agent_thinking_stream: Option<AgentMarkdownStreamState>,
    pub active_live: ActiveLiveSections,
    pub pending_planning_suggestion: Option<String>,
    pub pending_follow_up_messages: Vec<PendingFollowUpMessage>,
    pub queued_follow_up_messages: Vec<String>,
    pub running_tool_boundary_count: u64,
    pub terminal_focused: bool,
    pub state_db: Option<Arc<StateDb>>,
    pub state_db_status: Option<String>,
    pub running_task: Option<RunningTask>,
    pub repo_context_task: Option<JoinHandle<(Option<String>, Option<String>)>>,
    pub repo_slug: Option<String>,
    pub current_pr_url: Option<String>,
    pub codex_auth_mode: Option<SavedCodexAuthMode>,
    pub skill_picker_idx: usize,
    pub skill_picker_entries: Vec<SkillPickerEntry>,
    pub sandbox_network_access: Arc<AtomicBool>,
    pub permission_mode: PermissionMode,
    /// Currently active ralph loop goal, if any.
    pub goal: Option<RalphGoal>,
    /// Shared handle that model-facing goal tools write to.
    pub goal_handle: GoalHandle,
    /// Optional runtime event bus that mirrors AgentEvent to ACP/Wire
    /// subscribers. Set during TUI startup; None only in test contexts.
    pub event_bus: Option<Arc<RuntimeEventBus>>,
    /// MCP tool cache — populated on startup from configured MCP servers.
    pub mcp_tool_cache: Option<McpToolCache>,
}

#[derive(Debug, Clone)]
pub struct SkillPickerEntry {
    pub name: String,
    pub title: String,
    pub scope: String,
    pub display_path: String,
    pub enabled: bool,
    pub disable_model_invocation: bool,
}

/// Represents the lifecycle state of a ralph loop goal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalStatus {
    /// Agent is actively working toward the goal across turns.
    Pursuing,
    /// User paused the goal; can be resumed.
    Paused,
    /// Goal was completed successfully.
    Complete,
    /// Goal exceeded its configured token budget; soft-stop.
    BudgetLimited,
}

/// Tracks a long-running objective that the agent autonomously works toward.
#[derive(Clone, Debug)]
pub struct RalphGoal {
    /// The objective text set by `/goal <objective>`.
    pub objective: String,
    /// Current lifecycle status.
    pub status: GoalStatus,
    /// Optional token budget (input tokens). None = unlimited.
    pub token_budget: Option<u32>,
    /// Total input tokens consumed by goal turns.
    pub tokens_used: u32,
    /// Number of autonomous turns completed toward this goal.
    pub turns_completed: u32,
    /// Unix timestamp in seconds when the goal was created.
    pub created_at_epoch_seconds: u64,
}

impl RalphGoal {
    pub fn new(objective: String, token_budget: Option<u32>) -> Self {
        Self {
            objective,
            status: GoalStatus::Pursuing,
            token_budget,
            tokens_used: 0,
            turns_completed: 0,
            created_at_epoch_seconds: current_unix_timestamp_secs(),
        }
    }

    pub fn time_used_seconds(&self) -> u64 {
        current_unix_timestamp_secs().saturating_sub(self.created_at_epoch_seconds)
    }

    pub fn remaining_tokens(&self) -> Option<u32> {
        self.token_budget
            .map(|budget| budget.saturating_sub(self.tokens_used))
    }
}

pub fn current_unix_timestamp_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Shared handle for model-facing goal tools and TUI to observe/update goal state.
pub type GoalHandle = std::sync::Arc<std::sync::RwLock<Option<RalphGoal>>>;
