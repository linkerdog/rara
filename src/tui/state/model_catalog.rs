use super::*;

impl TuiApp {
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
        let registry = &self.config.provider_registry;
        results.retain(|preset| !registry.document.provider.contains_key(&preset.provider_id));
        for (provider_id, definition) in &registry.document.provider {
            for (model_id, model) in &definition.models {
                if !registry.model_allowed(provider_id, model_id) {
                    continue;
                }
                results.push(UnifiedModelPreset {
                    family: ProviderFamily::OpenAiCompatible,
                    provider_id: provider_id.clone(),
                    provider_label: definition
                        .name
                        .clone()
                        .unwrap_or_else(|| provider_id.clone()),
                    model_id: model_id.clone(),
                    model_label: model.name.clone().unwrap_or_else(|| model_id.clone()),
                    status: None,
                    context_window: model.limit.context,
                });
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
                if self
                    .config
                    .provider_registry
                    .document
                    .provider
                    .contains_key(&preset.provider_id)
                {
                    return self.config.provider_registry.available(&preset.provider_id);
                }
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

    pub fn selected_unified_preset_idx(&self) -> usize {
        let presets = self.all_unified_model_presets();
        presets
            .iter()
            .position(|p| {
                if let Some((provider, model)) = &self.config.provider_registry.selected
                    && self.config.selected_registry_model().is_some()
                {
                    return &p.provider_id == provider && &p.model_id == model;
                }
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
        PROVIDER_FAMILIES
            .get(self.provider_picker_idx)
            .map(|entry| entry.0)
            .unwrap_or(ProviderFamily::OpenAiCompatible)
    }

    pub fn registry_provider_ids(&self) -> Vec<String> {
        self.config
            .provider_registry
            .document
            .provider
            .keys()
            .filter(|id| self.config.provider_registry.provider_allowed(id))
            .cloned()
            .collect()
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

    pub(super) fn upsert_model_catalog_snapshot(
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

    pub(super) fn selected_model_preset(
        &self,
    ) -> Option<(&'static str, &'static str, &'static str)> {
        let presets = current_model_presets(self.provider_picker_idx);
        if presets.is_empty() {
            return None;
        }
        Some(presets[self.model_picker_idx.min(presets.len().saturating_sub(1))])
    }
}
