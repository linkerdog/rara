mod persistence;
mod state_presets;
#[cfg(test)]
mod tests;
mod transcript;
mod types;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use unicode_width::UnicodeWidthChar;

pub use self::state_presets::{
    current_model_presets, openai_compatible_preset_kind, selected_preset_idx_for_config,
    selected_provider_family_idx_for_config,
};
use self::types::CommittedTranscriptRenderCache;
pub use self::types::{
    ActiveLiveEvent, ActiveLiveSections, ActivePendingInteraction, ActivePendingInteractionKind,
    AgentMarkdownStreamState, CommandSpec, CompletedInteractionSnapshot, GoalHandle, GoalStatus,
    HelpTab, InteractionKind, ListPickerKind, LocalCommand, LocalCommandKind, OAuthLoginMode,
    OpenAiModelPickerAction, Overlay, PROVIDER_FAMILIES, PendingApprovalSnapshot,
    PendingInteractionSnapshot, PermissionMode, ProviderFamily, RalphGoal, RebuildSuccess,
    RunningTask, RuntimePhase, RuntimeSnapshot, SkillPickerEntry, StatusTab, TaskCompletion,
    TaskKind, TranscriptEntry, TranscriptEntryPayload, TranscriptTurn, TuiApp, TuiEvent,
};

const OPENAI_PROFILE_SETUP_KINDS: [OpenAiEndpointKind; 3] = [
    OpenAiEndpointKind::Custom,
    OpenAiEndpointKind::Kimi,
    OpenAiEndpointKind::Openrouter,
];
pub(super) const INPUT_HISTORY_LIMIT: usize = 200;

pub fn openai_profile_setup_kinds() -> &'static [OpenAiEndpointKind] {
    &OPENAI_PROFILE_SETUP_KINDS
}

use rara_provider_catalog::{ModelCatalogProvider, fallback_models};

use super::queued_input::PendingFollowUpMessage;
use crate::agent::{Agent, AgentExecutionMode, BashApprovalMode};
use crate::codex_model_catalog::{CodexModelOption, CodexReasoningOption};
use crate::config::{ConfigManager, DEFAULT_CODEX_BASE_URL, OpenAiEndpointKind};
use crate::redaction::redact_secrets;
use crate::state_db::StateDb;
use crate::tui::is_ssh_session;

fn completed_interaction_role(kind: InteractionKind, source: Option<&str>) -> &'static str {
    match kind {
        InteractionKind::Approval => "Shell Approval Completed",
        InteractionKind::PlanApproval => "Plan Decision",
        InteractionKind::RequestInput => match source {
            Some("plan_agent") => "Planning Question Answered",
            Some("explore_agent") => "Exploration Question Answered",
            Some(_) => "Sub-agent Question Answered",
            None => "Question Answered",
        },
    }
}

pub fn input_requests_command_palette(input: &str) -> bool {
    input.trim_start().starts_with('/')
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
    fn ensure_completed_interaction_entry(
        &mut self,
        kind: InteractionKind,
        title: &str,
        summary: &str,
        source: Option<&str>,
    ) {
        let role = completed_interaction_role(kind, source).to_string();
        let message = format!("{title}: {summary}");
        let exists = self
            .active_turn
            .entries
            .iter()
            .chain(
                self.committed_turns
                    .iter()
                    .flat_map(|turn| turn.entries.iter()),
            )
            .any(|entry| entry.role == role && entry.message == message);
        if !exists {
            self.active_turn
                .entries
                .push(TranscriptEntry::new(role, message));
        }
    }

    fn plan_approval_interaction(&self) -> PendingInteractionSnapshot {
        PendingInteractionSnapshot {
            kind: InteractionKind::PlanApproval,
            title: "Plan Ready".to_string(),
            summary: self
                .snapshot
                .plan_explanation
                .clone()
                .unwrap_or_else(|| "Review the proposed plan before implementation.".to_string()),
            options: Vec::new(),
            note: None,
            approval: None,
            source: None,
        }
    }

    fn set_plan_approval_interaction(&mut self, pending: bool) {
        self.snapshot
            .pending_interactions
            .retain(|item| item.kind != InteractionKind::PlanApproval);
        if pending {
            self.snapshot
                .pending_interactions
                .push(self.plan_approval_interaction());
        }
    }

    pub fn new(cm: ConfigManager) -> anyhow::Result<Self> {
        let mut cfg = cm.load()?;
        cfg.apply_provider_environment_defaults();
        let overlay = None;
        let startup_notice = startup_warning_for_config(&cfg);
        let provider_picker_idx = selected_provider_family_idx_for_config(&cfg);
        let model_picker_idx = selected_preset_idx_for_config(&cfg, provider_picker_idx);
        let sandbox_network = cfg.sandbox_workspace_write.network_access;
        Ok(Self {
            input: String::new(),
            input_cursor_offset: None,
            input_history: Vec::new(),
            input_history_cursor: None,
            input_history_draft: None,
            committed_turns: Vec::new(),
            active_turn: TranscriptTurn::default(),
            inserted_turns: 0,
            overlay,
            config: cfg,
            config_manager: cm,
            setup_status: None,
            notice: startup_notice,
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
            permission_picker_idx: 0,
            command_palette_idx: 0,
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
            recent_commands: Vec::new(),
            recent_threads: Vec::new(),
            resume_picker_idx: 0,
            committed_render_generation: 0,
            committed_render_cache: RefCell::new(CommittedTranscriptRenderCache::default()),
            transcript_scroll: 0,
            context_scroll: 0,
            terminal_width: 80,
            agent_markdown_stream: None,
            agent_thinking_stream: None,
            active_live: ActiveLiveSections::default(),
            pending_planning_suggestion: None,
            pending_follow_up_messages: Vec::new(),
            queued_follow_up_messages: Vec::new(),
            running_tool_boundary_count: 0,
            terminal_focused: true,
            state_db: None,
            state_db_status: None,
            running_task: None,
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
            viewport_was_cleared: false,
        })
    }

    pub fn start_repo_context_detection(&mut self) {
        if self.repo_context_task.is_some() {
            return;
        }

        self.repo_context_task = Some(tokio::task::spawn_blocking(detect_repo_context));
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
        self.running_task.is_some()
    }

    pub fn running_elapsed(&self) -> Option<std::time::Duration> {
        self.running_task
            .as_ref()
            .map(|task| task.started_at.elapsed())
    }

    pub fn current_model_label(&self) -> &str {
        self.config.model.as_deref().unwrap_or("-")
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
        selected_preset_idx_for_config(&self.config, self.provider_picker_idx)
    }

    pub fn selected_provider_family(&self) -> ProviderFamily {
        PROVIDER_FAMILIES[self.provider_picker_idx].0
    }

    pub fn current_model_picker_len(&self) -> usize {
        if self.selected_provider_family() == ProviderFamily::Codex {
            self.codex_model_options.len()
        } else if self.selected_provider_family() == ProviderFamily::DeepSeek {
            self.deepseek_model_options.len() + 1
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
        {
            if !options.iter().any(|model| model == current_model) {
                options.push(current_model.to_string());
            }
        }
        options.sort();
        options.dedup();
        self.deepseek_model_options = options;
        self.model_picker_idx = self.selected_preset_idx();
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

    pub fn openai_endpoint_kind_count(&self) -> usize {
        openai_profile_setup_kinds().len()
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
        if matches!(kind, OpenAiEndpointKind::Custom) {
            steps.push(Overlay::BaseUrlEditor);
        } else if self
            .config
            .base_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            steps.push(Overlay::BaseUrlEditor);
        }
        if !self.config.has_api_key() || matches!(kind, OpenAiEndpointKind::Custom) {
            steps.push(Overlay::ApiKeyEditor);
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
            Overlay::ApiKeyEditor,
            Overlay::ModelNameEditor,
        ];
        self.advance_openai_profile_setup();
    }

    pub fn advance_openai_profile_setup(&mut self) {
        if self.openai_setup_steps.is_empty() {
            self.openai_setup_keep_empty_api_key = false;
            self.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
            self.notice = Some(
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

    pub fn sync_snapshot(&mut self, agent: &Agent) {
        let runtime_context = agent.shared_runtime_context();
        let existing_plan_completion = self
            .completed_interaction(InteractionKind::PlanApproval)
            .cloned();
        let existing_pending_plan_approval = self.pending_plan_approval_interaction().cloned();
        let existing_local_request_completion = self
            .snapshot
            .completed_interactions
            .iter()
            .find(|item| {
                item.kind == InteractionKind::RequestInput && item.source.as_deref().is_some()
            })
            .cloned();
        let existing_local_request_inputs = self
            .snapshot
            .pending_interactions
            .iter()
            .filter(|item| {
                item.kind == InteractionKind::RequestInput && item.source.as_deref().is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut pending_interactions = Vec::new();
        if let Some(question) = agent.pending_user_input.as_ref() {
            pending_interactions.push(PendingInteractionSnapshot {
                kind: InteractionKind::RequestInput,
                title: question.question.clone(),
                summary: question.note.clone().unwrap_or_default(),
                options: question.options.clone(),
                note: question.note.clone(),
                approval: None,
                source: None,
            });
        }
        if agent.pending_user_input.is_none() {
            pending_interactions.extend(existing_local_request_inputs);
        }
        if let Some(item) = existing_pending_plan_approval {
            pending_interactions.push(item);
        }
        if let Some(pending) = agent.pending_approval.as_ref() {
            pending_interactions.push(PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "Pending Approval".to_string(),
                summary: pending.request.summary(),
                options: Vec::new(),
                note: None,
                approval: Some(PendingApprovalSnapshot {
                    tool_use_id: pending.tool_use_id.clone(),
                    command: pending.request.summary(),
                    allow_net: self
                        .sandbox_network_access
                        .load(std::sync::atomic::Ordering::Relaxed)
                        || pending.request.allow_net,
                    payload: pending.request.clone(),
                }),
                source: None,
            });
        }
        let mut completed_interactions = Vec::new();
        if let Some(item) = agent.completed_user_input.as_ref() {
            completed_interactions.push(CompletedInteractionSnapshot {
                kind: InteractionKind::RequestInput,
                title: item.title.clone(),
                summary: item.summary.clone(),
                source: None,
            });
        }
        if let Some(item) = agent.completed_approval.as_ref() {
            completed_interactions.push(CompletedInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: item.title.clone(),
                summary: item.summary.clone(),
                source: None,
            });
        }
        if let Some(item) = existing_local_request_completion {
            completed_interactions.push(item);
        }
        if let Some(item) = existing_plan_completion {
            completed_interactions.push(item);
        }
        for interaction in completed_interactions.iter() {
            self.ensure_completed_interaction_entry(
                interaction.kind,
                interaction.title.as_str(),
                interaction.summary.as_str(),
                interaction.source.as_deref(),
            );
        }
        let ext_counts = discover_extension_counts(&runtime_context.cwd);
        self.snapshot = RuntimeSnapshot {
            cwd: runtime_context.cwd,
            branch: runtime_context.branch,
            session_id: runtime_context.session_id,
            history_len: runtime_context.history_len,
            total_input_tokens: runtime_context.total_input_tokens,
            total_output_tokens: runtime_context.total_output_tokens,
            total_cache_hit_tokens: runtime_context.total_cache_hit_tokens,
            total_cache_miss_tokens: runtime_context.total_cache_miss_tokens,
            context_window_tokens: runtime_context.budget.context_window_tokens,
            compact_threshold_tokens: runtime_context.budget.compact_threshold_tokens,
            reserved_output_tokens: runtime_context.budget.reserved_output_tokens,
            stable_instructions_budget: runtime_context.budget.stable_instructions_budget,
            workspace_prompt_budget: runtime_context.budget.workspace_prompt_budget,
            active_turn_budget: runtime_context.budget.active_turn_budget,
            compacted_history_budget: runtime_context.budget.compacted_history_budget,
            retrieved_memory_budget: runtime_context.budget.retrieved_memory_budget,
            remaining_input_budget: runtime_context.budget.remaining_input_budget,
            estimated_history_tokens: runtime_context.compaction.estimated_history_tokens,
            compaction_count: runtime_context.compaction.compaction_count,
            last_compaction_before_tokens: runtime_context.compaction.last_compaction_before_tokens,
            last_compaction_after_tokens: runtime_context.compaction.last_compaction_after_tokens,
            last_compaction_recent_files: runtime_context.compaction.last_compaction_recent_files,
            last_compaction_boundary_version: runtime_context
                .compaction
                .last_compaction_boundary_version,
            last_compaction_boundary_before_tokens: runtime_context
                .compaction
                .last_compaction_boundary_before_tokens,
            last_compaction_boundary_recent_file_count: runtime_context
                .compaction
                .last_compaction_boundary_recent_file_count,
            compaction_source_entries: runtime_context.compaction.source_entries,
            plan_steps: runtime_context.plan.steps,
            plan_explanation: runtime_context.plan.explanation,
            pending_interactions,
            completed_interactions,
            todo_artifact_path: if agent.todo_state.is_some() {
                Some(
                    agent
                        .session_manager
                        .todo_file_path(&agent.session_id)
                        .display()
                        .to_string(),
                )
            } else {
                None
            },
            todo: runtime_context.todo,
            prompt_base_kind: runtime_context.prompt.base_prompt_kind,
            prompt_section_keys: runtime_context.prompt.section_keys,
            prompt_source_entries: runtime_context.prompt.source_entries,
            prompt_source_status_lines: runtime_context.prompt.source_status_lines,
            prompt_append_system_prompt: runtime_context.prompt.append_system_prompt,
            prompt_warnings: runtime_context.prompt.warnings,
            retrieval_source_entries: runtime_context.retrieval.entries,
            memory_selection: runtime_context.retrieval.memory_selection,
            assembly_entries: runtime_context.assembly.entries,
            // ── Extensions ──────────────────────────────────────
            // ── Extensions ──────────────────────────────────────
            extension_skill_count: agent.prompt_config().available_skills.len(),
            extension_skill_scopes: {
                let mut scopes: Vec<String> = agent
                    .prompt_config()
                    .available_skills
                    .iter()
                    .map(|s| s.scope.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                scopes.sort();
                scopes
            },
            extension_hook_count: ext_counts.0,
            extension_agent_count: ext_counts.1,
        };
        self.agent_execution_mode = agent.execution_mode;
        self.bash_approval_mode = agent.bash_approval_mode;
        self.populate_skill_picker_entries(agent);
        self.persist_runtime_state();
    }

    pub fn populate_skill_picker_entries(&mut self, agent: &Agent) {
        self.skill_picker_entries = agent
            .prompt_config()
            .available_skills
            .iter()
            .map(|s| SkillPickerEntry {
                name: s.name.clone(),
                title: s.title.clone().unwrap_or_else(|| s.name.clone()),
                scope: s.scope.clone(),
                display_path: s.display_path.clone(),
                enabled: !s.disable_model_invocation,
                disable_model_invocation: s.disable_model_invocation,
            })
            .collect();
        self.skill_picker_entries
            .sort_by(|a, b| a.name.cmp(&b.name));
    }

    pub fn open_overlay(&mut self, overlay: Overlay) {
        if matches!(overlay, Overlay::CommandPalette) {
            self.command_palette_idx = 0;
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::Provider)) {
            self.provider_picker_idx = selected_provider_family_idx_for_config(&self.config);
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::Resume)) {
            self.refresh_recent_threads_for_resume_picker();
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::Model)) {
            let selected_family = self.selected_provider_family();
            if matches!(selected_family, ProviderFamily::OpenAiCompatible) {
                if !matches!(
                    PROVIDER_FAMILIES
                        .get(selected_provider_family_idx_for_config(&self.config))
                        .map(|(family, _, _)| *family),
                    Some(ProviderFamily::OpenAiCompatible)
                ) {
                    self.config.set_provider("openai-compatible");
                }
                if self.config.active_openai_profile_kind() == Some(OpenAiEndpointKind::Deepseek) {
                    self.config.select_openai_profile(
                        OpenAiEndpointKind::Custom.default_profile_id(),
                        OpenAiEndpointKind::Custom.label(),
                        OpenAiEndpointKind::Custom,
                    );
                }
                self.model_picker_idx = 0;
            } else if let Some(provider) = self.single_provider_for_selected_family() {
                self.config.set_provider(provider.to_string());
                self.model_picker_idx = self.selected_preset_idx();
            }
            self.sync_reasoning_effort_picker();
        }
        if matches!(
            overlay,
            Overlay::ListPicker(ListPickerKind::OpenAiEndpointKind)
        ) {
            self.openai_endpoint_kind_picker_idx = self
                .selected_openai_profile_kind()
                .and_then(|kind| {
                    openai_profile_setup_kinds()
                        .iter()
                        .position(|candidate| *candidate == kind)
                })
                .unwrap_or(0);
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::OpenAiProfile)) {
            self.sync_openai_profile_picker();
        }
        if matches!(overlay, Overlay::BaseUrlEditor) {
            let provider_family = self.selected_provider_family();
            self.base_url_input = self.config.base_url.clone().unwrap_or_else(|| {
                if matches!(provider_family, ProviderFamily::OpenAiCompatible) {
                    self.config
                        .active_openai_profile_kind()
                        .unwrap_or(OpenAiEndpointKind::Custom)
                        .default_base_url()
                        .to_string()
                } else {
                    "http://localhost:11434".to_string()
                }
            });
            self.base_url_cursor_offset = None;
        }
        if matches!(overlay, Overlay::ApiKeyEditor) {
            self.api_key_input.clear();
            self.api_key_cursor_offset = None;
        }
        if matches!(overlay, Overlay::ModelNameEditor) {
            self.model_name_input = self.config.model.clone().unwrap_or_else(|| {
                self.selected_model_preset()
                    .map(|(_, _, default_model)| default_model.to_string())
                    .unwrap_or_default()
            });
            self.model_name_cursor_offset = None;
        }
        if matches!(overlay, Overlay::OpenAiProfileLabelEditor) {
            let kind = self
                .openai_profile_label_kind
                .or_else(|| self.selected_openai_profile_kind())
                .unwrap_or(OpenAiEndpointKind::Custom);
            self.openai_profile_label_input = format!("{} profile", kind.label());
            self.openai_profile_label_cursor_offset = None;
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::AuthMode)) {
            self.auth_mode_idx = if is_ssh_session() { 1 } else { 0 };
        }
        if matches!(
            overlay,
            Overlay::ListPicker(ListPickerKind::ReasoningEffort)
        ) {
            self.sync_reasoning_effort_picker();
        }
        if matches!(overlay, Overlay::SkillsPicker) {
            self.skill_picker_idx = 0;
        }
        if matches!(overlay, Overlay::Context) {
            self.context_scroll = 0;
        }
        self.overlay = Some(overlay);
    }

    pub fn sync_command_palette_with_input(&mut self) {
        if input_requests_command_palette(self.input.as_str()) {
            if matches!(self.overlay, None | Some(Overlay::CommandPalette)) {
                self.open_overlay(Overlay::CommandPalette);
            }
        } else if matches!(self.overlay, Some(Overlay::CommandPalette)) {
            self.overlay = None;
        }
    }

    pub fn set_agent_execution_mode(&mut self, mode: AgentExecutionMode) {
        self.agent_execution_mode = mode;
    }

    pub fn agent_execution_mode_label(&self) -> &'static str {
        match self.agent_execution_mode {
            AgentExecutionMode::Execute => "execute",
            AgentExecutionMode::Plan => "plan",
            AgentExecutionMode::Review => "review",
        }
    }

    pub fn permission_mode_label(&self) -> &'static str {
        self.permission_mode.label()
    }

    pub fn bash_approval_mode_label(&self) -> &'static str {
        match self.bash_approval_mode {
            BashApprovalMode::Always => "always",
            BashApprovalMode::Once => "once",
            BashApprovalMode::Suggestion => "suggestion",
        }
    }

    pub fn pending_question_option_label(&self, index: usize) -> Option<String> {
        self.pending_request_input()
            .and_then(|interaction| interaction.options.get(index))
            .map(|(label, _)| label.clone())
    }

    pub fn has_pending_approval(&self) -> bool {
        self.pending_command_approval().is_some()
    }

    pub fn has_pending_plan_approval(&self) -> bool {
        self.pending_plan_approval_interaction().is_some()
    }

    pub fn active_pending_interaction(&self) -> Option<ActivePendingInteraction<'_>> {
        if let Some(snapshot) = self.pending_plan_approval_interaction() {
            return Some(ActivePendingInteraction {
                kind: ActivePendingInteractionKind::PlanApproval,
                _snapshot: snapshot,
            });
        }
        if let Some(snapshot) = self.pending_command_approval() {
            return Some(ActivePendingInteraction {
                kind: ActivePendingInteractionKind::ShellApproval,
                _snapshot: snapshot,
            });
        }
        if let Some(snapshot) = self.pending_request_input() {
            let kind = match snapshot.source.as_deref() {
                Some("plan_agent") => ActivePendingInteractionKind::PlanningQuestion,
                Some("explore_agent") => ActivePendingInteractionKind::ExplorationQuestion,
                Some(_) => ActivePendingInteractionKind::SubAgentQuestion,
                None => ActivePendingInteractionKind::RequestInput,
            };
            return Some(ActivePendingInteraction {
                kind,
                _snapshot: snapshot,
            });
        }
        None
    }

    pub fn active_pending_option_count(&self) -> usize {
        let Some(pending) = self.active_pending_interaction() else {
            return 0;
        };
        match pending.kind {
            ActivePendingInteractionKind::PlanApproval => 2,
            ActivePendingInteractionKind::ShellApproval => 4,
            ActivePendingInteractionKind::PlanningQuestion
            | ActivePendingInteractionKind::ExplorationQuestion
            | ActivePendingInteractionKind::SubAgentQuestion
            | ActivePendingInteractionKind::RequestInput => self
                .pending_request_input()
                .map(|interaction| interaction.options.len().min(3))
                .unwrap_or(0),
        }
    }

    pub fn set_pending_plan_approval(&mut self, pending: bool) {
        self.set_plan_approval_interaction(pending);
        self.persist_runtime_state();
    }

    pub fn pending_request_input(&self) -> Option<&PendingInteractionSnapshot> {
        self.snapshot
            .pending_interactions
            .iter()
            .find(|item| item.kind == InteractionKind::RequestInput)
    }

    pub fn has_local_pending_request_input(&self) -> bool {
        self.pending_request_input()
            .and_then(|item| item.source.as_deref())
            .is_some()
    }

    pub fn pending_command_approval(&self) -> Option<&PendingInteractionSnapshot> {
        self.snapshot
            .pending_interactions
            .iter()
            .find(|item| item.kind == InteractionKind::Approval)
    }

    pub fn clear_pending_command_approval(&mut self) {
        self.snapshot
            .pending_interactions
            .retain(|item| item.kind != InteractionKind::Approval);
        self.persist_runtime_state();
    }

    pub fn pending_plan_approval_interaction(&self) -> Option<&PendingInteractionSnapshot> {
        self.snapshot
            .pending_interactions
            .iter()
            .find(|item| item.kind == InteractionKind::PlanApproval)
    }

    pub fn completed_interaction(
        &self,
        kind: InteractionKind,
    ) -> Option<&CompletedInteractionSnapshot> {
        self.snapshot
            .completed_interactions
            .iter()
            .find(|item| item.kind == kind)
    }

    pub fn record_completed_interaction(
        &mut self,
        kind: InteractionKind,
        title: impl Into<String>,
        summary: impl Into<String>,
        source: Option<String>,
    ) {
        let title = title.into();
        let summary = summary.into();
        self.snapshot
            .completed_interactions
            .retain(|item| item.kind != kind);
        self.snapshot
            .completed_interactions
            .push(CompletedInteractionSnapshot {
                kind,
                title: title.clone(),
                summary: summary.clone(),
                source: source.clone(),
            });
        self.ensure_completed_interaction_entry(
            kind,
            title.as_str(),
            summary.as_str(),
            source.as_deref(),
        );
        self.persist_runtime_state();
    }

    pub fn record_local_request_input(
        &mut self,
        source: impl Into<String>,
        title: impl Into<String>,
        options: Vec<(String, String)>,
        note: Option<String>,
    ) {
        self.snapshot.pending_interactions.retain(|item| {
            !(item.kind == InteractionKind::RequestInput && item.source.as_deref().is_some())
        });
        let title = title.into();
        self.snapshot
            .pending_interactions
            .push(PendingInteractionSnapshot {
                kind: InteractionKind::RequestInput,
                title: title.clone(),
                summary: note.clone().unwrap_or_default(),
                options,
                note,
                approval: None,
                source: Some(source.into()),
            });
        self.notice = Some(format!("{title}"));
        self.persist_runtime_state();
    }

    pub fn clear_local_request_input(&mut self) {
        self.snapshot.pending_interactions.retain(|item| {
            !(item.kind == InteractionKind::RequestInput && item.source.as_deref().is_some())
        });
        self.persist_runtime_state();
    }

    pub fn close_overlay(&mut self) {
        if matches!(
            self.overlay,
            Some(Overlay::BaseUrlEditor | Overlay::ApiKeyEditor | Overlay::ModelNameEditor)
        ) {
            self.cancel_openai_profile_setup();
        }
        self.overlay = match self.overlay {
            Some(Overlay::ListPicker(ListPickerKind::OpenAiEndpointKind)) => {
                Some(Overlay::ListPicker(ListPickerKind::Model))
            }
            Some(Overlay::ListPicker(ListPickerKind::OpenAiProfile)) => {
                Some(Overlay::ListPicker(ListPickerKind::Model))
            }
            Some(Overlay::BaseUrlEditor) => Some(Overlay::ListPicker(ListPickerKind::Model)),
            Some(Overlay::ApiKeyEditor) => {
                if self.config.provider == "codex" {
                    Some(Overlay::ListPicker(ListPickerKind::AuthMode))
                } else {
                    Some(Overlay::ListPicker(ListPickerKind::Model))
                }
            }
            Some(Overlay::ModelNameEditor) => Some(Overlay::ListPicker(ListPickerKind::Model)),
            Some(Overlay::OpenAiProfileLabelEditor) => {
                Some(Overlay::ListPicker(ListPickerKind::OpenAiProfile))
            }
            Some(Overlay::ListPicker(ListPickerKind::AuthMode)) => {
                if self.codex_model_options.is_empty() {
                    Some(Overlay::ListPicker(ListPickerKind::Provider))
                } else {
                    Some(Overlay::ListPicker(ListPickerKind::Model))
                }
            }
            _ => None,
        };
    }
}

fn discover_extension_counts(cwd: &str) -> (usize, usize) {
    let root = std::path::Path::new(cwd);
    let mut hr = crate::hooks::HookRegistry::new();
    let mut ar = crate::agents_ext::AgentRegistry::new();
    hr.discover_repo_hooks(root);
    ar.discover_repo_agents(root);
    (hr.hooks.len(), ar.agents.len())
}

mod helpers;
pub(crate) use helpers::*;
