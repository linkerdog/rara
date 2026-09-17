use super::*;

impl TuiApp {
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

    pub(super) fn openai_profile_setup_sequence(&self) -> Vec<Overlay> {
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

    pub(super) fn sync_openai_profile_picker(&mut self) {
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
}
