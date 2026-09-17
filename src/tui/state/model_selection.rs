use super::*;

impl TuiApp {
    pub fn select_unified_model(&mut self, idx: usize) {
        let presets = self.all_unified_model_presets();
        let Some(preset) = presets.get(idx).cloned() else {
            return;
        };
        if self
            .config
            .provider_registry
            .document
            .provider
            .contains_key(&preset.provider_id)
        {
            if let Err(error) = self
                .config
                .select_registry_model(&preset.provider_id, &preset.model_id)
            {
                self.push_notice(error.to_string());
            }
            self.provider_picker_idx = selected_provider_family_idx_for_config(&self.config);
            return;
        }

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

    pub(super) fn single_provider_for_selected_family(&self) -> Option<&'static str> {
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
