use bottom_pane_model::BottomPaneModel;
mod bottom_pane_model;
mod overlay_state;
mod pending_interaction;
mod persistence;
mod planning_lifecycle;
mod runtime_snapshot;
mod shared_tasks;
mod state_presets;
#[cfg(test)]
mod tests;
mod transcript;
mod types;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use unicode_width::UnicodeWidthChar;

pub use self::planning_lifecycle::{
    PlanningApprovalDecision, PlanningApprovalStatus, PlanningLifecycleSnapshot,
};
pub use self::state_presets::{
    current_model_presets, openai_compatible_preset_kind, selected_preset_idx_for_config,
    selected_provider_family_idx_for_config,
};
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
use crate::oauth::OAuthManager;
pub(crate) use crate::runtime_client::RebuildSuccess;

const OPENAI_PROFILE_SETUP_KINDS: [OpenAiEndpointKind; 4] = [
    OpenAiEndpointKind::Custom,
    OpenAiEndpointKind::Kimi,
    OpenAiEndpointKind::KimiCoding,
    OpenAiEndpointKind::Openrouter,
];

pub(super) const INPUT_HISTORY_LIMIT: usize = 200;

pub fn openai_profile_setup_kinds() -> &'static [OpenAiEndpointKind] {
    &OPENAI_PROFILE_SETUP_KINDS
}

fn terminal_multiplexer_label(
    multiplexer: Option<&rara_terminal_detection::Multiplexer>,
) -> String {
    match multiplexer {
        Some(rara_terminal_detection::Multiplexer::Tmux { version }) => version
            .as_ref()
            .filter(|value| !value.is_empty())
            .map(|version| format!("tmux/{version}"))
            .unwrap_or_else(|| "tmux".to_string()),
        Some(rara_terminal_detection::Multiplexer::Zellij) => "zellij".to_string(),
        None => "-".to_string(),
    }
}

fn terminal_remote_label(remote: Option<&rara_terminal_detection::RemoteSession>) -> &'static str {
    match remote {
        Some(rara_terminal_detection::RemoteSession::Ssh) => "ssh",
        None => "local",
    }
}

use rara_persistence::redaction::redact_secrets;
use rara_provider_catalog::ModelCatalogEntry;
use rara_provider_catalog::{ModelCatalogProvider, fallback_models};
use rara_state::state_db::StateDb;

use super::queued_input::PendingFollowUpMessage;
use crate::agent::{AgentExecutionMode, BashApprovalMode};
use crate::codex_model_catalog::{CodexModelOption, CodexReasoningOption};
use crate::config::{ConfigManager, DEFAULT_CODEX_BASE_URL, OpenAiEndpointKind};

pub fn input_requests_command_palette(input: &str) -> bool {
    let trimmed = input.trim_start();
    // Open the palette when the user types a bare '/' or the start of a
    // command name.  Once a space (argument) appears, close it so Enter
    // goes to Submit instead of ApplyOverlaySelection.
    trimmed.starts_with('/') && !trimmed.contains(|c: char| c.is_whitespace())
}

pub(crate) fn contains_structured_planning_output(message: &str) -> bool {
    message.contains("<proposed_plan>")
        || message.contains("<plan>")
        || message.contains("<request_user_input>")
}

fn state_db_status_error(prefix: &str, message: impl Into<String>) -> String {
    format!("{prefix}: {}", redact_secrets(message.into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextInputTarget {
    Composer,
    BaseUrl,
    ApiKey,
    ModelName,
    OpenAiProfileLabel,
}

fn effective_cursor_offset(text: &str, cursor_offset: Option<usize>) -> usize {
    cursor_offset
        .unwrap_or_else(|| text.chars().count())
        .min(text.chars().count())
}

pub(crate) fn char_offset_to_byte_index(text: &str, char_offset: usize) -> usize {
    if char_offset == 0 {
        return 0;
    }

    text.char_indices()
        .nth(char_offset)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

pub(super) fn composer_display_char_width(ch: char) -> usize {
    match ch {
        '\t' => 4,
        _ => UnicodeWidthChar::width(ch).unwrap_or(0),
    }
}

fn startup_warning_for_config(config: &crate::config::RaraConfig) -> Option<String> {
    if config.provider == "codex" {
        return None;
    }
    if !config.has_api_key() && super::provider_requires_api_key(&config.provider) {
        Some(format!(
            "Warning: {} is missing an API key. Use /model to configure the current provider.",
            config.provider
        ))
    } else {
        None
    }
}

mod composer;

impl TuiApp {
    pub fn new(cm: ConfigManager) -> anyhow::Result<Self> {
        let mut cfg = cm.load()?;
        cfg.apply_provider_environment_defaults();
        crate::tui::theme::install_config(&cfg.tui.theme);
        let overlay = None;
        let startup_notice = startup_warning_for_config(&cfg);
        let provider_picker_idx = selected_provider_family_idx_for_config(&cfg);
        let model_picker_idx = selected_preset_idx_for_config(&cfg, provider_picker_idx);
        let sandbox_network = cfg.sandbox_workspace_write.network_access;
        let mut app = Self {
            bottom_pane: BottomPaneModel {
                input: String::new(),
                input_cursor_offset: None,
                notice: startup_notice,
                ..Default::default()
            },
            input_history: Vec::new(),
            input_history_cursor: None,
            input_history_draft: None,
            committed_turns: Vec::new(),
            active_turn: TranscriptTurn::default(),
            overlay,
            overlay_stack: Vec::new(),
            sidebar_visible: true,
            thinking_collapsed: false,
            config: cfg,
            config_manager: cm,
            setup_status: None,
            runtime_phase: RuntimePhase::Idle,
            runtime_phase_detail: None,
            snapshot: RuntimeSnapshot::default(),
            agent_execution_mode: AgentExecutionMode::Execute,
            bash_approval_mode: BashApprovalMode::Suggestion,
            provider_picker_idx,
            model_picker_idx,
            openai_endpoint_kind_picker_idx: 0,
            openai_profile_picker_idx: 0,
            reasoning_effort_picker_idx: 0,
            auth_mode_idx: 0,
            nowledge_mem_picker_idx: 0,
            approval_picker_idx: 0,
            permission_picker_idx: 0,
            command_palette_idx: 0,
            model_search_query: String::new(),
            model_search_idx: 0,
            base_url_input: String::new(),
            base_url_cursor_offset: None,
            api_key_input: String::new(),
            api_key_cursor_offset: None,
            model_name_input: String::new(),
            model_name_cursor_offset: None,
            openai_profile_label_input: String::new(),
            openai_profile_label_cursor_offset: None,
            openai_profile_label_kind: None,
            openai_setup_steps: Vec::new(),
            openai_setup_keep_empty_api_key: false,
            codex_model_options: Vec::new(),
            deepseek_model_options: fallback_models(ModelCatalogProvider::DeepSeek),
            kimi_model_options: fallback_models(ModelCatalogProvider::Kimi),
            deepseek_model_context_windows: rara_provider_catalog::fallback_catalog(
                ModelCatalogProvider::DeepSeek,
            )
            .into_iter()
            .filter_map(|entry| entry.context_window.map(|window| (entry.id, window)))
            .collect(),
            kimi_model_context_windows: rara_provider_catalog::fallback_catalog(
                ModelCatalogProvider::Kimi,
            )
            .into_iter()
            .filter_map(|entry| entry.context_window.map(|window| (entry.id, window)))
            .collect(),
            recent_commands: Vec::new(),
            recent_threads: Vec::new(),
            resume_picker_idx: 0,
            resume_sort_by_created: false,
            resume_search_query: String::new(),
            committed_render_generation: 0,
            committed_render_cache: RefCell::new(CommittedTranscriptRenderCache::default()),
            transcript_scroll: 0,
            transcript_selection: crate::tui::selection::TranscriptSelection::default(),
            context_scroll: 0,
            terminal_width: 80,
            agent_markdown_stream: None,
            agent_thinking_stream: None,
            active_live: ActiveLiveSections::default(),
            running_tool_boundary_count: 0,
            terminal_focused: true,
            state_db: None,
            state_db_status: None,
            shared_task_root: None,
            shared_task_fingerprint: None,
            shared_task_last_poll: None,
            mcp_manager: None,
            lsp_manager: None,
            #[cfg(test)]
            prompt_source_registry: None,
            #[cfg(test)]
            skill_source_registry: None,
            #[cfg(test)]
            hook_registry: None,
            hook_runtime: None,
            explicit_plugin_dirs: Vec::new(),
            memory_handler: None,
            provider_connection_status: std::collections::HashMap::new(),
            repo_context_task: None,
            repo_slug: None,
            current_pr_url: None,
            codex_auth_mode: None,
            skill_picker_idx: 0,
            skill_picker_entries: Vec::new(),
            sandbox_network_access: Arc::new(AtomicBool::new(sandbox_network)),
            permission_mode: PermissionMode::Auto,
            goal: None,
            goal_handle: Arc::new(std::sync::RwLock::new(None)),
            event_bus: None,
            mcp_tool_cache: None,
        };

        app.set_deepseek_model_catalog_with_source(
            rara_provider_catalog::fallback_catalog(ModelCatalogProvider::DeepSeek),
            true,
        );
        app.set_kimi_model_catalog_with_source(
            rara_provider_catalog::fallback_catalog(ModelCatalogProvider::Kimi),
            true,
        );
        app.refresh_provider_connection_status();
        app.refresh_recent_threads();

        Ok(app)
    }

    pub fn start_repo_context_detection(&mut self) {
        if self.repo_context_task.is_some() {
            return;
        }

        self.repo_context_task = Some(tokio::task::spawn_blocking(detect_repo_context));
    }

    pub fn refresh_provider_connection_status(&mut self) {
        let mut status = std::collections::HashMap::new();

        for (family, _, _) in PROVIDER_FAMILIES.iter() {
            let connected = match family {
                ProviderFamily::Codex => {
                    let has_key = self.config.provider == "codex" && self.config.has_api_key();
                    let has_state = self
                        .config
                        .provider_states
                        .get("codex")
                        .and_then(|s| s.api_key.as_ref())
                        .is_some();
                    let has_oauth = OAuthManager::new()
                        .ok()
                        .and_then(|m| m.has_saved_auth().ok())
                        == Some(true);
                    has_key || has_state || has_oauth
                }
                ProviderFamily::DeepSeek => {
                    let has_key = self.config.provider == "deepseek" && self.config.has_api_key();
                    let has_profile = self.config.openai_profiles.values().any(|p| {
                        p.kind == crate::config::OpenAiEndpointKind::Deepseek
                            && p.api_key.as_ref().is_some()
                    });
                    has_key || has_profile
                }
                ProviderFamily::Kimi => {
                    let has_key = self.config.provider == "kimi" && self.config.has_api_key();
                    let has_profile = self.config.openai_profiles.values().any(|p| {
                        p.kind == crate::config::OpenAiEndpointKind::Kimi
                            && p.api_key.as_ref().is_some()
                    });
                    has_key || has_profile
                }
                ProviderFamily::KimiCoding => {
                    let has_key =
                        self.config.provider == "kimi-coding" && self.config.has_api_key();
                    let has_profile = self.config.openai_profiles.values().any(|profile| {
                        profile.kind == crate::config::OpenAiEndpointKind::KimiCoding
                            && profile.api_key.as_ref().is_some()
                    });
                    has_key || has_profile
                }
                ProviderFamily::Gemini => {
                    let has_key = self.config.provider == "gemini" && self.config.has_api_key();
                    let has_state = self
                        .config
                        .provider_states
                        .get("gemini")
                        .and_then(|state| state.api_key.as_ref())
                        .is_some();
                    let has_oauth = rara_config::ensure_rara_home_dir()
                        .ok()
                        .and_then(|dir| crate::google_oauth::GoogleOAuthManager::new(dir).ok())
                        .is_some_and(|manager| manager.has_saved_auth());
                    has_key || has_state || has_oauth
                }
                ProviderFamily::OpenAiCompatible => self
                    .config
                    .openai_profiles
                    .values()
                    .any(|p| p.api_key.as_ref().is_some()),
                ProviderFamily::Bedrock => {
                    // Bedrock is often configured via env vars, but also check config.
                    self.config.provider == "bedrock"
                }
                ProviderFamily::Ollama => {
                    // connected if we have a base_url.
                    self.config
                        .base_url
                        .as_deref()
                        .is_some_and(|url| !url.is_empty())
                }
                ProviderFamily::CandleLocal => true, // Local is always "connected"
            };
            status.insert(*family, connected);
        }

        self.provider_connection_status = status;
    }

    pub async fn finish_repo_context_task_if_ready(&mut self) {
        let should_finish = self
            .repo_context_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if !should_finish {
            return;
        }

        let handle = self
            .repo_context_task
            .take()
            .expect("repo context task should exist");
        if let Ok((repo_slug, current_pr_url)) = handle.await {
            self.repo_slug = repo_slug;
            self.current_pr_url = current_pr_url;
        }
    }

    pub fn is_busy(&self) -> bool {
        self.bottom_pane.running_task.is_some()
    }

    pub fn running_elapsed(&self) -> Option<std::time::Duration> {
        self.bottom_pane
            .running_task
            .as_ref()
            .map(|task| task.started_at.elapsed())
    }

    pub fn current_model_label(&self) -> &str {
        self.config.model.as_deref().unwrap_or("-")
    }

    pub fn model_routing_view(&self) -> ModelRoutingView {
        let surface = self.config.effective_provider_surface();
        let main_model = surface
            .model
            .value
            .unwrap_or_else(|| self.current_model_label())
            .to_string();
        if let Some(auxiliary_model) = surface
            .auxiliary_model
            .value
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            return ModelRoutingView {
                main_model,
                main_source: surface.model.source.label().to_string(),
                auxiliary_model: auxiliary_model.to_string(),
                auxiliary_source: surface.auxiliary_model.source.label().to_string(),
                auxiliary_route: "configured".to_string(),
                auxiliary_uses_main_model: false,
            };
        }

        if let Some(auxiliary_model) = self.inferred_auxiliary_model(&main_model) {
            return ModelRoutingView {
                main_model,
                main_source: surface.model.source.label().to_string(),
                auxiliary_model,
                auxiliary_source: "inferred".to_string(),
                auxiliary_route: "provider_lite".to_string(),
                auxiliary_uses_main_model: false,
            };
        }

        ModelRoutingView {
            auxiliary_model: main_model.clone(),
            main_model,
            main_source: surface.model.source.label().to_string(),
            auxiliary_source: "main_model".to_string(),
            auxiliary_route: "fallback".to_string(),
            auxiliary_uses_main_model: true,
        }
    }

    fn inferred_auxiliary_model(&self, main_model: &str) -> Option<String> {
        let endpoint_kind = self.config.active_openai_profile_kind()?;
        crate::llm::infer_openai_compatible_auxiliary_model(main_model, endpoint_kind)
            .map(|model| model.into_owned())
    }

    pub fn terminal_diagnostics_view(&self) -> TerminalDiagnosticsView {
        let info = rara_terminal_detection::terminal_info();
        TerminalDiagnosticsView {
            name: format!("{:?}", info.name),
            user_agent: info.user_agent_token(),
            term_program: info.term_program.clone(),
            term: info.term.clone(),
            multiplexer: terminal_multiplexer_label(info.multiplexer.as_ref()),
            remote: terminal_remote_label(info.remote.as_ref()).to_string(),
            history_mode: if info.is_zellij() {
                "zellij-fallback-insert".to_string()
            } else {
                "scroll-region".to_string()
            },
            focused: self.terminal_focused,
            width_columns: self.terminal_width,
        }
    }

    pub fn repo_context_hint(&self) -> Option<String> {
        let branch = self.snapshot.branch.trim();
        let mut parts = Vec::new();

        if let Some(repo_slug) = self
            .repo_slug
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            parts.push(format!("repo: {repo_slug}"));
        }

        if !branch.is_empty() {
            parts.push(format!("branch: {branch}"));
        }

        if let Some(pr_url) = self
            .current_pr_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            parts.push(format!("PR: {pr_url}"));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("  "))
        }
    }

    pub fn selected_preset_idx(&self) -> usize {
        if self.selected_provider_family() == ProviderFamily::Codex
            && !self.codex_model_options.is_empty()
        {
            return self
                .codex_model_options
                .iter()
                .position(|preset| self.config.model.as_deref() == Some(preset.model.as_str()))
                .or_else(|| {
                    self.codex_model_options
                        .iter()
                        .position(|preset| preset.is_default)
                })
                .unwrap_or(0);
        }
        if self.selected_provider_family() == ProviderFamily::DeepSeek {
            return self
                .deepseek_model_options
                .iter()
                .position(|model| self.config.model.as_deref() == Some(model.as_str()))
                .unwrap_or(0);
        }
        if self.selected_provider_family() == ProviderFamily::Kimi {
            return self
                .kimi_model_options
                .iter()
                .position(|model| self.config.model.as_deref() == Some(model.as_str()))
                .unwrap_or(0);
        }
        selected_preset_idx_for_config(&self.config, self.provider_picker_idx)
    }

    pub fn all_unified_model_presets(&self) -> Vec<UnifiedModelPreset> {
        use crate::tui::state::state_presets::{
            BEDROCK_MODEL_PRESETS, LOCAL_MODEL_PRESETS, OLLAMA_MODEL_PRESETS,
        };

        let mut results = Vec::new();

        for (family, name, _description) in PROVIDER_FAMILIES.iter() {
            match family {
                ProviderFamily::Codex => {
                    if self.codex_model_options.is_empty() {
                        // Default if not connected/cached
                        results.push(UnifiedModelPreset {
                            family: *family,
                            provider_id: "codex".into(),
                            provider_label: "Codex".into(),
                            model_id: "gpt-4o".into(),
                            model_label: "gpt-4o".into(),
                            status: None,
                            context_window: None,
                        });
                    } else {
                        for opt in &self.codex_model_options {
                            results.push(UnifiedModelPreset {
                                family: *family,
                                provider_id: name.to_lowercase(),
                                provider_label: name.to_string(),
                                model_id: opt.id.clone(),
                                model_label: opt.label.clone(),
                                status: None,
                                context_window: None,
                            });
                        }
                    }
                }
                ProviderFamily::DeepSeek => {
                    if self.deepseek_model_options.is_empty() {
                        // Default if not connected/cached
                        results.push(UnifiedModelPreset {
                            family: *family,
                            provider_id: "deepseek".into(),
                            provider_label: "DeepSeek".into(),
                            model_id: "deepseek-chat".into(),
                            model_label: "deepseek-chat".into(),
                            status: None,
                            context_window: None,
                        });
                    } else {
                        for model in &self.deepseek_model_options {
                            results.push(UnifiedModelPreset {
                                family: *family,
                                provider_id: "deepseek".to_string(),
                                provider_label: "DeepSeek".to_string(),
                                model_id: model.clone(),
                                model_label: model.clone(),
                                status: None,
                                context_window: self.model_context_window(*family, model),
                            });
                        }
                    }
                }
                ProviderFamily::Kimi => {
                    if self.kimi_model_options.is_empty() {
                        results.push(UnifiedModelPreset {
                            family: *family,
                            provider_id: "kimi".into(),
                            provider_label: "Moonshot AI".into(),
                            model_id: "kimi-k2.6".into(),
                            model_label: "kimi-k2.6".into(),
                            status: None,
                            context_window: None,
                        });
                    } else {
                        for model in &self.kimi_model_options {
                            results.push(UnifiedModelPreset {
                                family: *family,
                                provider_id: "kimi".to_string(),
                                provider_label: "Moonshot AI".to_string(),
                                model_id: model.clone(),
                                model_label: model.clone(),
                                status: None,
                                context_window: self.model_context_window(*family, model),
                            });
                        }
                    }
                }
                ProviderFamily::KimiCoding => {
                    results.push(UnifiedModelPreset {
                        family: *family,
                        provider_id: "kimi-coding".into(),
                        provider_label: "Kimi For Coding".into(),
                        model_id: crate::config::DEFAULT_KIMI_CODING_MODEL.into(),
                        model_label: crate::config::DEFAULT_KIMI_CODING_MODEL.into(),
                        status: None,
                        context_window: None,
                    });
                }
                ProviderFamily::OpenAiCompatible => {
                    let mut found_profile = false;
                    for (profile_id, profile) in &self.config.openai_profiles {
                        // Skip profiles whose kind already has a dedicated
                        // provider family (these show up via their own branch).
                        if matches!(
                            profile.kind,
                            OpenAiEndpointKind::Deepseek
                                | OpenAiEndpointKind::Kimi
                                | OpenAiEndpointKind::KimiCoding
                        ) {
                            continue;
                        }
                        found_profile = true;
                        let model_id = profile
                            .model
                            .clone()
                            .unwrap_or_else(|| profile.kind.default_model().to_string());
                        results.push(UnifiedModelPreset {
                            family: *family,
                            provider_id: profile_id.clone(),
                            provider_label: profile.label.clone(),
                            model_id: model_id.clone(),
                            model_label: model_id,
                            status: None,
                            context_window: None,
                        });
                    }

                    // If no profiles, show templates
                    if !found_profile {
                        use crate::tui::state::state_presets::OPENAI_COMPATIBLE_MODEL_PRESETS;
                        for preset in OPENAI_COMPATIBLE_MODEL_PRESETS.iter() {
                            results.push(UnifiedModelPreset {
                                family: *family,
                                provider_id: "openai-compatible".to_string(),
                                provider_label: "OpenAI".to_string(),
                                model_id: preset.2.to_string(),
                                model_label: preset.0.to_string(),
                                status: None,
                                context_window: None,
                            });
                        }
                    }
                }
                ProviderFamily::CandleLocal => {
                    for preset in LOCAL_MODEL_PRESETS.iter() {
                        results.push(UnifiedModelPreset {
                            family: *family,
                            provider_id: "gemma4".to_string(),
                            provider_label: "Local".to_string(),
                            model_id: preset.2.to_string(),
                            model_label: preset.0.to_string(),
                            status: Some("alpha".to_string()),
                            context_window: None,
                        });
                    }
                }
                ProviderFamily::Ollama => {
                    for preset in OLLAMA_MODEL_PRESETS.iter() {
                        results.push(UnifiedModelPreset {
                            family: *family,
                            provider_id: "ollama".to_string(),
                            provider_label: "Ollama".to_string(),
                            model_id: preset.2.to_string(),
                            model_label: preset.0.to_string(),
                            status: None,
                            context_window: None,
                        });
                    }
                }
                ProviderFamily::Bedrock => {
                    for preset in BEDROCK_MODEL_PRESETS.iter() {
                        results.push(UnifiedModelPreset {
                            family: *family,
                            provider_id: "bedrock".to_string(),
                            provider_label: "Bedrock".to_string(),
                            model_id: preset.2.to_string(),
                            model_label: preset.0.to_string(),
                            status: None,
                            context_window: None,
                        });
                    }
                }
                ProviderFamily::Gemini => {
                    results.push(UnifiedModelPreset {
                        family: *family,
                        provider_id: "gemini".to_string(),
                        provider_label: "Gemini".to_string(),
                        model_id: "gemini-3-flash".to_string(),
                        model_label: "Gemini 3 Flash".to_string(),
                        status: None,
                        context_window: None,
                    });
                }
            }
        }
        for p in &mut results {
            if p.context_window.is_none() {
                p.context_window = self.model_context_window(p.family, &p.model_id);
            }
        }
        results
    }

    /// Returns models whose provider currently has a usable configured connection.
    ///
    /// This is a compatibility projection until the runtime publishes provider
    /// availability alongside its model catalogs.
    pub fn available_unified_model_presets(&self) -> Vec<UnifiedModelPreset> {
        self.all_unified_model_presets()
            .into_iter()
            .filter(|preset| {
                self.provider_connection_status
                    .get(&preset.family)
                    .copied()
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Look up context window tokens for a model from provider catalogs.
    pub fn model_context_window(&self, family: ProviderFamily, model_id: &str) -> Option<u32> {
        match family {
            ProviderFamily::DeepSeek => self.deepseek_model_context_windows.get(model_id),
            ProviderFamily::Kimi => self.kimi_model_context_windows.get(model_id),
            ProviderFamily::KimiCoding => None,
            _ => None,
        }
        .copied()
    }

    pub fn select_unified_model(&mut self, idx: usize) {
        let presets = self.all_unified_model_presets();
        let Some(preset) = presets.get(idx).cloned() else {
            return;
        };

        // Update provider picker index to match the selected model's family.
        if let Some(family_idx) = PROVIDER_FAMILIES
            .iter()
            .position(|(family, _, _)| *family == preset.family)
        {
            self.provider_picker_idx = family_idx;
        }

        match preset.family {
            ProviderFamily::Codex => {
                self.config.set_provider("codex");
                self.config.set_model(Some(preset.model_id));
                self.config.set_revision(None);
                if crate::config::should_reset_codex_base_url(self.config.base_url.as_deref()) {
                    self.config
                        .set_base_url(Some(DEFAULT_CODEX_BASE_URL.to_string()));
                }
                self.sync_reasoning_effort_picker();
            }
            ProviderFamily::DeepSeek => {
                self.config.select_openai_profile(
                    OpenAiEndpointKind::Deepseek.default_profile_id(),
                    OpenAiEndpointKind::Deepseek.label(),
                    OpenAiEndpointKind::Deepseek,
                );
                self.config.set_provider("deepseek");
                self.config.set_model(Some(preset.model_id));
                self.config.set_revision(None);
            }
            ProviderFamily::OpenAiCompatible => {
                // If it's a user profile, select it.
                if let Some(profile) = self.config.openai_profiles.get(&preset.provider_id) {
                    self.config.select_openai_profile(
                        profile.id.clone(),
                        profile.label.clone(),
                        profile.kind,
                    );
                    self.config.set_model(Some(preset.model_id));
                } else {
                    // It's a template preset (e.g. "openai-compatible")
                    self.config.set_provider("openai-compatible");
                    // Use model_id to infer kind if possible, but mainly we just want to trigger setup
                }
                self.config.set_revision(None);
            }
            ProviderFamily::Gemini => {
                self.config.set_provider("gemini");
                self.config.set_model(Some(preset.model_id));
                self.config.set_base_url(None);
                self.config.set_revision(None);
            }
            _ => {
                self.config.set_provider(preset.provider_id.clone());
                self.config.set_model(Some(preset.model_id));
                self.config.set_revision(None);

                if preset.provider_id == "ollama"
                    && self
                        .config
                        .base_url
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .is_none()
                {
                    self.config
                        .set_base_url(Some("http://localhost:11434".to_string()));
                }
            }
        }
    }

    pub fn selected_unified_preset_idx(&self) -> usize {
        let presets = self.all_unified_model_presets();
        presets
            .iter()
            .position(|p| {
                p.provider_id == self.config.provider
                    && self.config.model.as_deref() == Some(&p.model_id)
            })
            .unwrap_or(0)
    }

    pub fn first_unified_preset_idx_for_family(&self, target_family: ProviderFamily) -> usize {
        let presets = self.all_unified_model_presets();
        presets
            .iter()
            .position(|p| p.family == target_family)
            .unwrap_or(0)
    }

    pub fn selected_provider_family(&self) -> ProviderFamily {
        PROVIDER_FAMILIES[self.provider_picker_idx].0
    }

    pub fn current_model_picker_len(&self) -> usize {
        if self.selected_provider_family() == ProviderFamily::Codex {
            self.codex_model_options.len()
        } else if self.selected_provider_family() == ProviderFamily::DeepSeek {
            self.deepseek_model_options.len() + 1
        } else if self.selected_provider_family() == ProviderFamily::Kimi {
            self.kimi_model_options.len() + 1
        } else if self.selected_provider_family() == ProviderFamily::OpenAiCompatible {
            self.openai_model_picker_profiles().len()
        } else {
            current_model_presets(self.provider_picker_idx).len()
        }
    }

    pub fn deepseek_api_key_action_idx(&self) -> usize {
        self.deepseek_model_options.len()
    }

    pub fn selected_deepseek_api_key_action(&self) -> bool {
        self.selected_provider_family() == ProviderFamily::DeepSeek
            && self.model_picker_idx >= self.deepseek_api_key_action_idx()
    }

    pub fn kimi_api_key_action_idx(&self) -> usize {
        self.kimi_model_options.len()
    }

    pub fn selected_kimi_api_key_action(&self) -> bool {
        self.selected_provider_family() == ProviderFamily::Kimi
            && self.model_picker_idx >= self.kimi_api_key_action_idx()
    }

    pub fn selected_codex_model(&self) -> Option<&CodexModelOption> {
        self.codex_model_options.get(
            self.model_picker_idx
                .min(self.codex_model_options.len().saturating_sub(1)),
        )
    }

    pub fn selected_codex_reasoning_options(&self) -> &[CodexReasoningOption] {
        self.selected_codex_model()
            .map(|preset| preset.reasoning_options.as_slice())
            .unwrap_or(&[])
    }

    pub fn current_reasoning_effort_label(&self) -> String {
        let current = self
            .config
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(option) = self
            .selected_codex_reasoning_options()
            .iter()
            .find(|option| Some(option.value.as_str()) == current)
        {
            return option.label.clone();
        }
        current
            .map(crate::codex_model_catalog::reasoning_effort_label)
            .unwrap_or("default")
            .to_string()
    }

    pub fn sync_reasoning_effort_picker(&mut self) {
        let options = self.selected_codex_reasoning_options();
        let selected = self
            .config
            .reasoning_effort
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        self.reasoning_effort_picker_idx = options
            .iter()
            .position(|option| Some(option.value.as_str()) == selected)
            .or_else(|| options.iter().position(|option| option.is_default))
            .unwrap_or(0);
    }

    #[cfg(test)]
    pub fn set_codex_model_options(&mut self, options: Vec<CodexModelOption>) {
        self.codex_model_options = options;
        self.model_picker_idx = self.selected_preset_idx();
        self.sync_reasoning_effort_picker();
    }

    pub fn set_deepseek_model_options(&mut self, options: Vec<String>) {
        let mut options = if options.is_empty() {
            fallback_models(ModelCatalogProvider::DeepSeek)
        } else {
            options
        };
        if let Some(current_model) = self
            .config
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            && !options.iter().any(|model| model == current_model)
        {
            options.push(current_model.to_string());
        }
        options.sort();
        options.dedup();
        self.deepseek_model_options = options;
        self.model_picker_idx = self.selected_preset_idx();
    }

    pub fn set_deepseek_model_catalog(&mut self, catalog: Vec<ModelCatalogEntry>) {
        self.set_deepseek_model_catalog_with_source(catalog, false);
    }

    pub fn set_deepseek_model_catalog_with_source(
        &mut self,
        catalog: Vec<ModelCatalogEntry>,
        is_fallback: bool,
    ) {
        self.deepseek_model_context_windows = catalog
            .iter()
            .filter_map(|entry| {
                entry
                    .context_window
                    .map(|window| (entry.id.clone(), window))
            })
            .collect();
        self.set_deepseek_model_options(catalog.iter().map(|entry| entry.id.clone()).collect());
        self.upsert_model_catalog_snapshot("deepseek", catalog, is_fallback);
    }

    pub fn set_kimi_model_options(&mut self, options: Vec<String>) {
        let mut options = if options.is_empty() {
            fallback_models(ModelCatalogProvider::Kimi)
        } else {
            options
        };
        if let Some(current_model) = self
            .config
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            && !options.iter().any(|model| model == current_model)
        {
            options.push(current_model.to_string());
        }
        options.sort();
        options.dedup();
        self.kimi_model_options = options;
        self.model_picker_idx = self.selected_preset_idx();
    }

    pub fn set_kimi_model_catalog(&mut self, catalog: Vec<ModelCatalogEntry>) {
        self.set_kimi_model_catalog_with_source(catalog, false);
    }

    pub fn set_kimi_model_catalog_with_source(
        &mut self,
        catalog: Vec<ModelCatalogEntry>,
        is_fallback: bool,
    ) {
        self.kimi_model_context_windows = catalog
            .iter()
            .filter_map(|entry| {
                entry
                    .context_window
                    .map(|window| (entry.id.clone(), window))
            })
            .collect();
        self.set_kimi_model_options(catalog.iter().map(|entry| entry.id.clone()).collect());
        self.upsert_model_catalog_snapshot("kimi", catalog, is_fallback);
    }

    fn upsert_model_catalog_snapshot(
        &mut self,
        provider_id: &str,
        models: Vec<ModelCatalogEntry>,
        is_fallback: bool,
    ) {
        if let Some(snapshot) = self
            .snapshot
            .model_catalogs
            .iter_mut()
            .find(|snapshot| snapshot.provider_id == provider_id)
        {
            *snapshot = ModelCatalogSnapshot {
                provider_id: provider_id.to_string(),
                models,
                is_fallback,
            };
        } else {
            self.snapshot.model_catalogs.push(ModelCatalogSnapshot {
                provider_id: provider_id.to_string(),
                models,
                is_fallback,
            });
        }
    }

    pub fn apply_model_catalog_snapshots(&mut self, catalogs: &[ModelCatalogSnapshot]) {
        for catalog in catalogs {
            match catalog.provider_id.as_str() {
                "deepseek" => {
                    self.deepseek_model_context_windows = catalog
                        .models
                        .iter()
                        .filter_map(|entry| {
                            entry
                                .context_window
                                .map(|window| (entry.id.clone(), window))
                        })
                        .collect();
                    self.set_deepseek_model_options(
                        catalog
                            .models
                            .iter()
                            .map(|entry| entry.id.clone())
                            .collect(),
                    );
                }
                "kimi" => {
                    self.kimi_model_context_windows = catalog
                        .models
                        .iter()
                        .filter_map(|entry| {
                            entry
                                .context_window
                                .map(|window| (entry.id.clone(), window))
                        })
                        .collect();
                    self.set_kimi_model_options(
                        catalog
                            .models
                            .iter()
                            .map(|entry| entry.id.clone())
                            .collect(),
                    );
                }
                _ => {}
            }
        }
    }

    fn selected_model_preset(&self) -> Option<(&'static str, &'static str, &'static str)> {
        let presets = current_model_presets(self.provider_picker_idx);
        if presets.is_empty() {
            return None;
        }
        Some(presets[self.model_picker_idx.min(presets.len().saturating_sub(1))])
    }

    pub fn selected_openai_profile_kind(&self) -> Option<OpenAiEndpointKind> {
        if self.selected_provider_family() != ProviderFamily::OpenAiCompatible {
            return None;
        }
        self.selected_openai_model_picker_profile()
            .map(|profile| profile.kind)
            .or_else(|| self.config.active_openai_profile_kind())
            .filter(|kind| *kind != OpenAiEndpointKind::Deepseek)
            .or(Some(OpenAiEndpointKind::Custom))
    }

    pub fn selected_openai_model_picker_action(&self) -> Option<OpenAiModelPickerAction> {
        if self.selected_provider_family() != ProviderFamily::OpenAiCompatible {
            return None;
        }
        if self
            .openai_model_picker_profiles()
            .get(self.model_picker_idx)
            .is_some()
        {
            Some(OpenAiModelPickerAction::SelectProfile)
        } else {
            None
        }
    }

    pub fn openai_profile_needs_setup(&self) -> bool {
        if self.selected_provider_family() != ProviderFamily::OpenAiCompatible {
            return false;
        }
        let missing_api = !self.config.has_api_key();
        let missing_base_url = self
            .config
            .base_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty());
        let missing_model = self
            .config
            .model
            .as_deref()
            .is_none_or(|value| value.trim().is_empty());
        missing_api || missing_base_url || missing_model
    }

    pub fn selected_openai_setup_kind(&self) -> OpenAiEndpointKind {
        openai_profile_setup_kinds()
            .get(
                self.openai_endpoint_kind_picker_idx
                    .min(openai_profile_setup_kinds().len().saturating_sub(1)),
            )
            .copied()
            .unwrap_or(OpenAiEndpointKind::Custom)
    }

    fn openai_profile_setup_sequence(&self) -> Vec<Overlay> {
        let kind = self
            .selected_openai_profile_kind()
            .unwrap_or(OpenAiEndpointKind::Custom);
        let mut steps = Vec::new();
        if matches!(kind, OpenAiEndpointKind::Custom)
            || self
                .config
                .base_url
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            steps.push(Overlay::BaseUrlEditor);
        }
        if !self.config.has_api_key() || matches!(kind, OpenAiEndpointKind::Custom) {
            steps.push(Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible));
        }
        if matches!(kind, OpenAiEndpointKind::Custom)
            || self
                .config
                .model
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            steps.push(Overlay::ModelNameEditor);
        }
        steps
    }

    pub fn begin_openai_profile_setup(&mut self) {
        self.openai_setup_steps.clear();
        self.openai_setup_keep_empty_api_key = false;
        self.openai_profile_label_kind = None;
        self.open_overlay(Overlay::ListPicker(ListPickerKind::OpenAiEndpointKind));
    }

    pub fn begin_active_openai_profile_setup(&mut self) {
        self.openai_setup_keep_empty_api_key = false;
        self.openai_setup_steps = self.openai_profile_setup_sequence();
        self.advance_openai_profile_setup();
    }

    pub fn begin_created_openai_profile_setup(&mut self) {
        self.openai_setup_keep_empty_api_key = false;
        let mut steps = self.openai_profile_setup_sequence();
        if !steps.contains(&Overlay::ModelNameEditor) {
            steps.push(Overlay::ModelNameEditor);
        }
        self.openai_setup_steps = steps;
        self.advance_openai_profile_setup();
    }

    pub fn begin_edit_openai_profile_setup(&mut self) {
        self.openai_setup_keep_empty_api_key = true;
        self.openai_setup_steps = vec![
            Overlay::BaseUrlEditor,
            Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible),
            Overlay::ModelNameEditor,
        ];
        self.advance_openai_profile_setup();
    }

    pub fn advance_openai_profile_setup(&mut self) {
        if self.openai_setup_steps.is_empty() {
            self.openai_setup_keep_empty_api_key = false;
            self.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
            self.bottom_pane.notice = Some(
                "Endpoint setup complete. Review the active profile and press Enter to rebuild."
                    .into(),
            );
            return;
        }
        let next = self.openai_setup_steps.remove(0);
        self.open_overlay(next);
    }

    pub fn cancel_openai_profile_setup(&mut self) {
        self.openai_setup_steps.clear();
        self.openai_setup_keep_empty_api_key = false;
    }

    pub fn set_openai_setup_kind(&mut self, kind: OpenAiEndpointKind) {
        self.openai_profile_label_kind = Some(kind);
        self.open_overlay(Overlay::OpenAiProfileLabelEditor);
    }

    pub fn selected_openai_profiles(&self) -> Vec<(String, String)> {
        let Some(kind) = self.selected_openai_profile_kind() else {
            return Vec::new();
        };
        let mut profiles = self
            .config
            .openai_profiles
            .values()
            .filter(|profile| profile.kind == kind)
            .map(|profile| (profile.id.clone(), profile.label.clone()))
            .collect::<Vec<_>>();
        profiles.sort_by(|left, right| {
            left.1
                .to_ascii_lowercase()
                .cmp(&right.1.to_ascii_lowercase())
                .then_with(|| left.0.cmp(&right.0))
        });
        profiles
    }

    pub fn openai_model_picker_profiles(&self) -> Vec<&crate::config::OpenAiEndpointProfile> {
        let active_id = self.config.active_openai_profile_id();
        let mut profiles = self
            .config
            .openai_profiles
            .values()
            .filter(|profile| profile.kind != OpenAiEndpointKind::Deepseek)
            .collect::<Vec<_>>();
        profiles.sort_by(|left, right| {
            let left_active = Some(left.id.as_str()) == active_id;
            let right_active = Some(right.id.as_str()) == active_id;
            right_active
                .cmp(&left_active)
                .then_with(|| left.kind.label().cmp(right.kind.label()))
                .then_with(|| {
                    left.label
                        .to_ascii_lowercase()
                        .cmp(&right.label.to_ascii_lowercase())
                })
                .then_with(|| left.id.cmp(&right.id))
        });
        profiles
    }

    pub fn selected_openai_model_picker_profile(
        &self,
    ) -> Option<crate::config::OpenAiEndpointProfile> {
        if self.selected_provider_family() != ProviderFamily::OpenAiCompatible {
            return None;
        }
        self.openai_model_picker_profiles()
            .get(self.model_picker_idx)
            .map(|profile| (*profile).clone())
    }

    pub fn select_openai_model_picker_profile(&mut self) -> Option<String> {
        let profile = self.selected_openai_model_picker_profile()?;
        let label = profile.label.clone();
        self.config
            .select_openai_profile(profile.id, profile.label, profile.kind);
        Some(label)
    }

    pub fn delete_active_openai_profile(&mut self) -> Option<String> {
        if self.selected_provider_family() != ProviderFamily::OpenAiCompatible {
            return None;
        }
        if self.config.openai_profiles.len() <= 1 {
            return None;
        }
        let active_id = self.config.active_openai_profile_id()?.to_string();
        let next = self
            .openai_model_picker_profiles()
            .into_iter()
            .find(|profile| profile.id != active_id)?
            .clone();
        self.config
            .select_openai_profile(next.id, next.label, next.kind);
        let deleted = self.config.openai_profiles.remove(active_id.as_str())?;
        self.model_picker_idx = 0;
        Some(deleted.label)
    }

    fn sync_openai_profile_picker(&mut self) {
        let profiles = self.selected_openai_profiles();
        self.openai_profile_picker_idx = self
            .config
            .active_openai_profile_id()
            .and_then(|active_id| profiles.iter().position(|(id, _)| id == active_id))
            .map(|idx| idx + 1)
            .unwrap_or(0);
    }

    pub(crate) fn next_openai_profile_id(&self, kind: OpenAiEndpointKind, label: &str) -> String {
        let mut slug = label
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>();
        while slug.contains("--") {
            slug = slug.replace("--", "-");
        }
        slug = slug.trim_matches('-').to_string();
        if slug.is_empty() {
            slug = "profile".to_string();
        }
        let prefix = match kind {
            OpenAiEndpointKind::Custom => "custom",
            OpenAiEndpointKind::Deepseek => "deepseek",
            OpenAiEndpointKind::Kimi => "kimi",
            OpenAiEndpointKind::KimiCoding => "kimi-coding",
            OpenAiEndpointKind::Openrouter => "openrouter",
        };
        let base = format!("{prefix}-{slug}");
        if !self.config.openai_profiles.contains_key(base.as_str()) {
            return base;
        }
        let mut suffix = 2;
        loop {
            let candidate = format!("{base}-{suffix}");
            if !self.config.openai_profiles.contains_key(candidate.as_str()) {
                return candidate;
            }
            suffix += 1;
        }
    }

    fn single_provider_for_selected_family(&self) -> Option<&'static str> {
        if self.selected_provider_family() == ProviderFamily::Codex {
            return Some("codex");
        }
        if self.selected_provider_family() == ProviderFamily::DeepSeek {
            return None;
        }
        if self.selected_provider_family() == ProviderFamily::Kimi {
            return None;
        }
        if self.selected_provider_family() == ProviderFamily::OpenAiCompatible {
            return None;
        }
        let presets = current_model_presets(self.provider_picker_idx);
        let provider = presets.first()?.1;
        if presets
            .iter()
            .all(|(_, preset_provider, _)| *preset_provider == provider)
        {
            Some(provider)
        } else {
            None
        }
    }

    pub fn select_local_model(&mut self, idx: usize) {
        self.model_picker_idx = idx;
        if self.selected_provider_family() == ProviderFamily::Codex {
            let Some(preset) = self.selected_codex_model().cloned() else {
                return;
            };
            self.config.set_provider("codex");
            self.config.set_model(Some(preset.model));
            self.config.set_revision(None);
            if crate::config::should_reset_codex_base_url(self.config.base_url.as_deref()) {
                self.config
                    .set_base_url(Some(DEFAULT_CODEX_BASE_URL.to_string()));
            }
            self.sync_reasoning_effort_picker();
            return;
        }
        if self.selected_provider_family() == ProviderFamily::DeepSeek {
            let Some(model) = self.deepseek_model_options.get(idx).cloned() else {
                return;
            };
            self.config.select_openai_profile(
                OpenAiEndpointKind::Deepseek.default_profile_id(),
                OpenAiEndpointKind::Deepseek.label(),
                OpenAiEndpointKind::Deepseek,
            );
            self.config.set_provider("deepseek");
            self.config.set_model(Some(model));
            self.config.set_revision(None);
            return;
        }
        if self.selected_provider_family() == ProviderFamily::Kimi {
            let Some(model) = self.kimi_model_options.get(idx).cloned() else {
                return;
            };
            self.config.select_openai_profile(
                OpenAiEndpointKind::Kimi.default_profile_id(),
                OpenAiEndpointKind::Kimi.label(),
                OpenAiEndpointKind::Kimi,
            );
            self.config.set_model(Some(model));
            self.config.set_revision(None);
            return;
        }

        let presets = current_model_presets(self.provider_picker_idx);
        if idx >= presets.len() {
            return;
        }
        let (_, provider, model) = presets[idx];
        if self.selected_provider_family() == ProviderFamily::OpenAiCompatible {
            let kind = openai_compatible_preset_kind(idx);
            let (profile_id, label) = self
                .config
                .active_openai_profile()
                .filter(|profile| profile.kind == kind)
                .map(|profile| (profile.id.clone(), profile.label.clone()))
                .unwrap_or_else(|| {
                    (
                        kind.default_profile_id().to_string(),
                        kind.label().to_string(),
                    )
                });
            self.config.select_openai_profile(profile_id, label, kind);
            self.config.set_revision(None);
            return;
        }
        self.config.set_provider(provider.to_string());
        if provider == "ollama" {
            self.config.set_model(Some(model.to_string()));
            self.config.set_revision(None);
            if self
                .config
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                self.config
                    .set_base_url(Some("http://localhost:11434".to_string()));
            }
        } else if provider == "codex" {
            self.config.set_model(Some(model.to_string()));
            self.config.set_revision(None);
            if crate::config::should_reset_codex_base_url(self.config.base_url.as_deref()) {
                self.config
                    .set_base_url(Some(DEFAULT_CODEX_BASE_URL.to_string()));
            }
        } else if provider == "openai-compatible" {
            if self
                .config
                .model
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                self.config.set_model(Some(model.to_string()));
            }
            self.config.set_revision(None);
            if self
                .config
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                self.config
                    .set_base_url(Some("https://api.openai.com/v1".to_string()));
            }
        } else {
            self.config.set_model(Some(model.to_string()));
            self.config.set_revision(Some("main".to_string()));
            self.config.set_base_url(None);
        }
    }

    pub fn cycle_local_model(&mut self) {
        let len = self.current_model_picker_len();
        if len == 0 {
            return;
        }
        let next = (self.selected_preset_idx() + 1) % len;
        self.select_local_model(next);
    }

    pub fn apply_selected_codex_reasoning_effort(&mut self) {
        let selected = self
            .selected_codex_reasoning_options()
            .get(
                self.reasoning_effort_picker_idx.min(
                    self.selected_codex_reasoning_options()
                        .len()
                        .saturating_sub(1),
                ),
            )
            .map(|option| option.value.clone())
            .or_else(|| {
                self.selected_codex_model()
                    .and_then(|preset| preset.default_reasoning_effort.clone())
            });
        self.config.set_reasoning_effort(selected);
    }
}

mod helpers;
pub(crate) use helpers::*;
