use super::*;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct RaraConfig {
    pub provider: String,
    #[serde(
        default,
        serialize_with = "serialize_secret_option",
        deserialize_with = "deserialize_secret_option"
    )]
    pub api_key: Option<SecretString>,
    #[serde(skip)]
    pub runtime_api_key: Option<SecretString>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    #[serde(default, alias = "utility_model")]
    pub auxiliary_model: Option<String>,
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "MultiAgentPolicy::is_default")]
    pub multi_agent_policy: MultiAgentPolicy,
    pub reasoning_summary: Option<String>,
    pub revision: Option<String>,
    pub thinking: Option<bool>,
    pub num_ctx: Option<u32>,
    pub aws_region: Option<String>,
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
    pub append_system_prompt: Option<String>,
    pub append_system_prompt_file: Option<String>,
    pub compact_prompt: Option<String>,
    pub compact_prompt_file: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_states: BTreeMap<String, ProviderConfigState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_openai_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub openai_profiles: BTreeMap<String, OpenAiEndpointProfile>,
    #[serde(
        default,
        skip_serializing_if = "SandboxWorkspaceWriteConfig::is_default"
    )]
    pub sandbox_workspace_write: SandboxWorkspaceWriteConfig,
    #[serde(default, skip_serializing_if = "MemoryConsolidationConfig::is_default")]
    pub memory_consolidation: MemoryConsolidationConfig,
    #[serde(default, skip_serializing_if = "ContextFileSearchPolicy::is_default")]
    pub context_file_search: ContextFileSearchPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugin_dirs: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "BuiltinPluginConfig::is_default")]
    pub builtin_plugins: BuiltinPluginConfig,
    #[serde(default, skip_serializing_if = "TuiConfig::is_default")]
    pub tui: TuiConfig,
}

impl RaraConfig {
    pub fn is_openai_compatible_family(provider: &str) -> bool {
        OpenAiEndpointKind::from_legacy_provider(provider).is_some()
    }

    pub fn api_key(&self) -> Option<&str> {
        self.runtime_api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .map(SecretString::expose_secret)
    }

    pub fn api_key_secret(&self) -> Option<SecretString> {
        self.runtime_api_key
            .clone()
            .or_else(|| self.api_key.clone())
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key().is_some_and(|value| !value.is_empty())
    }

    pub fn set_api_key(&mut self, value: impl Into<String>) {
        self.runtime_api_key = None;
        self.api_key = Some(SecretString::from(value.into()));
        self.sync_active_provider_state();
    }

    pub fn clear_api_key(&mut self) {
        self.runtime_api_key = None;
        self.api_key = None;
        self.sync_active_provider_state();
    }

    pub fn apply_provider_environment_defaults(&mut self) {
        self.apply_provider_environment_defaults_from(|key| std::env::var(key).ok());
    }

    pub fn apply_provider_environment_defaults_from<F>(&mut self, mut read_env: F)
    where
        F: FnMut(&str) -> Option<String>,
    {
        if self.has_api_key() {
            return;
        }
        let env_key = match self.effective_openai_endpoint_kind() {
            Some(OpenAiEndpointKind::Kimi) => "MOONSHOT_API_KEY",
            Some(OpenAiEndpointKind::KimiCoding) => "KIMI_API_KEY",
            _ => return,
        };
        if let Some(value) = read_env(env_key)
            && !value.trim().is_empty()
        {
            self.runtime_api_key = Some(SecretString::from(value));
        }
    }

    pub fn clear_provider_api_key(&mut self, provider: &str) {
        if let Some(kind) = OpenAiEndpointKind::from_legacy_provider(provider) {
            if self.provider == provider
                || (self.provider == "openai-compatible"
                    && self.active_openai_profile_kind() == Some(kind))
            {
                self.clear_api_key();
            } else if let Some(profile) = self.openai_profiles.get_mut(kind.default_profile_id()) {
                profile.api_key = None;
            }
            return;
        }
        if self.provider == provider {
            self.clear_api_key();
            return;
        }
        if let Some(state) = self.provider_states.get_mut(provider) {
            state.api_key = None;
        }
    }

    pub fn set_provider_api_key(&mut self, provider: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(kind) = OpenAiEndpointKind::from_legacy_provider(provider) {
            if self.provider == provider
                || (self.provider == "openai-compatible"
                    && self.active_openai_profile_kind() == Some(kind))
            {
                self.set_api_key(value);
            } else {
                let mut profile = self.profile_for_kind_or_default(kind);
                profile.api_key = Some(SecretString::from(value));
                self.openai_profiles.insert(profile.id.clone(), profile);
            }
            return;
        }
        if self.provider == provider {
            self.set_api_key(value);
            return;
        }
        self.provider_states
            .entry(provider.to_string())
            .or_default()
            .api_key = Some(SecretString::from(value));
    }

    pub fn set_provider(&mut self, provider: impl Into<String>) {
        self.sync_active_provider_state();
        let provider = provider.into();
        if provider != "openai-compatible"
            && let Some(kind) = OpenAiEndpointKind::from_legacy_provider(provider.as_str())
        {
            self.provider = "openai-compatible".to_string();
            self.reset_provider_scoped_fields();
            let profile = self.profile_for_kind_or_default(kind);
            self.active_openai_profile_id = Some(profile.id.clone());
            self.openai_profiles
                .insert(profile.id.clone(), profile.clone());
            self.apply_openai_profile(profile);
            self.apply_provider_environment_defaults();
            return;
        }
        self.provider = provider;
        self.reset_provider_scoped_fields();
        if self.provider == "openai-compatible" {
            let profile = self
                .active_openai_profile()
                .cloned()
                .unwrap_or_else(|| self.profile_for_kind_or_default(OpenAiEndpointKind::Custom));
            self.active_openai_profile_id = Some(profile.id.clone());
            self.openai_profiles
                .insert(profile.id.clone(), profile.clone());
            self.apply_openai_profile(profile);
        } else if let Some(state) = self.provider_states.get(&self.provider).cloned() {
            self.apply_provider_state(state);
        }
    }

    pub fn set_base_url(&mut self, value: Option<String>) {
        self.base_url = normalize_optional_string(value);
        self.sync_active_provider_state();
    }

    pub fn set_model(&mut self, value: Option<String>) {
        self.model = normalize_optional_string(value);
        self.sync_active_provider_state();
    }

    pub fn set_auxiliary_model(&mut self, value: Option<String>) {
        self.auxiliary_model = normalize_optional_string(value);
        self.sync_active_provider_state();
    }

    pub fn set_reasoning_effort(&mut self, value: Option<String>) {
        self.reasoning_effort = normalize_optional_string(value);
        self.sync_active_provider_state();
    }

    pub fn set_reasoning_summary(&mut self, value: Option<String>) {
        self.reasoning_summary = normalize_reasoning_summary(value);
        self.sync_active_provider_state();
    }

    pub fn set_revision(&mut self, value: Option<String>) {
        self.revision = normalize_optional_string(value);
        self.sync_active_provider_state();
    }

    pub fn set_thinking(&mut self, value: Option<bool>) {
        self.thinking = value;
        self.sync_active_provider_state();
    }

    pub fn set_num_ctx(&mut self, value: Option<u32>) {
        self.num_ctx = value;
        self.sync_active_provider_state();
    }

    pub fn apply_codex_defaults(&mut self) {
        self.apply_codex_defaults_for_base_url(DEFAULT_CODEX_BASE_URL);
    }

    pub fn apply_codex_defaults_for_base_url(&mut self, base_url: &str) {
        if should_apply_codex_base_url(self.base_url.as_deref(), base_url) {
            self.set_base_url(Some(base_url.to_string()));
        }
        if should_reset_codex_model(self.model.as_deref()) {
            self.set_model(Some(DEFAULT_CODEX_MODEL.to_string()));
        }
    }

    pub fn migrate_legacy_provider_state(&mut self) {
        self.reasoning_summary =
            migrate_reasoning_summary(self.reasoning_summary.take(), self.thinking);
        for state in self.provider_states.values_mut() {
            state.reasoning_summary =
                migrate_reasoning_summary(state.reasoning_summary.take(), state.thinking);
        }
        self.migrate_legacy_openai_profiles();
    }

    /// Hardcoded base URL for built-in providers. Returns the default API
    /// endpoint for known provider IDs. Custom/openai-compatible providers
    /// return None (they have their base_url set via profiles).
    fn provider_hardcoded_base_url(provider: &str) -> Option<&'static str> {
        match provider {
            "deepseek" => Some(DEFAULT_DEEPSEEK_BASE_URL),
            "gemini" => Some(DEFAULT_GEMINI_BASE_URL),
            "kimi" => Some(DEFAULT_KIMI_BASE_URL),
            "kimi-coding" => Some(DEFAULT_KIMI_CODING_BASE_URL),
            _ => None,
        }
    }

    pub fn effective_provider_surface(&self) -> EffectiveProviderSurface<'_> {
        let provider_state = if self.provider == "openai-compatible" {
            None
        } else {
            self.provider_states.get(&self.provider)
        };
        let profile = if self.provider == "openai-compatible" {
            self.active_openai_profile()
        } else {
            None
        };
        EffectiveProviderSurface {
            provider: self.provider.as_str(),
            model: resolve_provider_value(
                provider_state
                    .and_then(|state| state.model.as_deref())
                    .or_else(|| profile.and_then(|profile| profile.model.as_deref())),
                self.model.as_deref(),
                None,
                None,
            ),
            auxiliary_model: resolve_provider_value(
                provider_state
                    .and_then(|state| state.auxiliary_model.as_deref())
                    .or_else(|| profile.and_then(|profile| profile.auxiliary_model.as_deref())),
                self.auxiliary_model.as_deref(),
                None,
                None,
            ),
            base_url: resolve_provider_value(
                provider_state
                    .and_then(|state| state.base_url.as_deref())
                    .or_else(|| profile.and_then(|profile| profile.base_url.as_deref())),
                self.base_url.as_deref(),
                None,
                Self::provider_hardcoded_base_url(self.provider.as_str()),
            ),
            revision: resolve_provider_value(
                provider_state
                    .and_then(|state| state.revision.as_deref())
                    .or_else(|| profile.and_then(|profile| profile.revision.as_deref())),
                self.revision.as_deref(),
                None,
                None,
            ),
            reasoning_effort: resolve_provider_value(
                provider_state
                    .and_then(|state| state.reasoning_effort.as_deref())
                    .or_else(|| profile.and_then(|profile| profile.reasoning_effort.as_deref())),
                self.reasoning_effort.as_deref(),
                None,
                None,
            ),
            reasoning_summary: resolve_provider_value(
                provider_state
                    .and_then(|state| state.reasoning_summary.as_deref())
                    .or_else(|| profile.and_then(|profile| profile.reasoning_summary.as_deref())),
                self.reasoning_summary.as_deref(),
                None,
                Some(DEFAULT_REASONING_SUMMARY),
            ),
            api_key: resolve_provider_value(
                provider_state
                    .and_then(|state| state.api_key.as_ref().map(SecretString::expose_secret))
                    .or_else(|| {
                        profile.and_then(|profile| {
                            profile.api_key.as_ref().map(SecretString::expose_secret)
                        })
                    }),
                self.api_key.as_ref().map(SecretString::expose_secret),
                self.runtime_api_key
                    .as_ref()
                    .map(SecretString::expose_secret),
                None,
            ),
        }
    }

    pub fn active_openai_profile_id(&self) -> Option<&str> {
        self.active_openai_profile_id
            .as_deref()
            .filter(|id| self.openai_profiles.contains_key(*id))
            .or_else(|| self.openai_profiles.keys().next().map(String::as_str))
    }

    pub fn active_openai_profile(&self) -> Option<&OpenAiEndpointProfile> {
        let id = self.active_openai_profile_id()?;
        self.openai_profiles.get(id)
    }

    pub fn active_openai_profile_label(&self) -> Option<&str> {
        self.active_openai_profile()
            .map(|profile| profile.label.as_str())
    }

    pub fn active_openai_profile_kind(&self) -> Option<OpenAiEndpointKind> {
        self.active_openai_profile().map(|profile| profile.kind)
    }

    fn effective_openai_endpoint_kind(&self) -> Option<OpenAiEndpointKind> {
        if self.provider == "openai-compatible" {
            return self.active_openai_profile_kind();
        }
        OpenAiEndpointKind::from_legacy_provider(self.provider.as_str())
    }

    pub fn select_openai_profile(
        &mut self,
        profile_id: impl Into<String>,
        label: impl Into<String>,
        kind: OpenAiEndpointKind,
    ) {
        self.sync_active_provider_state();
        self.provider = "openai-compatible".to_string();
        self.reset_provider_scoped_fields();

        let profile_id = profile_id.into();
        let label = label.into();
        let mut profile = self
            .openai_profiles
            .get(&profile_id)
            .cloned()
            .unwrap_or_else(|| self.default_openai_profile(&profile_id, label.as_str(), kind));
        profile.id = profile_id.clone();
        if profile.label.trim().is_empty() {
            profile.label = label;
        }
        profile.kind = kind;
        if profile
            .base_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            profile.base_url = Some(kind.default_base_url().to_string());
        }
        if profile
            .model
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            profile.model = Some(kind.default_model().to_string());
        }
        self.active_openai_profile_id = Some(profile_id.clone());
        self.openai_profiles.insert(profile_id, profile.clone());
        self.apply_openai_profile(profile);
        self.apply_provider_environment_defaults();
    }

    fn sync_active_provider_state(&mut self) {
        if self.provider.trim().is_empty() {
            return;
        }
        if self.provider == "openai-compatible" {
            self.sync_active_openai_profile();
            return;
        }
        self.provider_states
            .insert(self.provider.clone(), self.current_provider_state());
    }

    fn current_provider_state(&self) -> ProviderConfigState {
        ProviderConfigState {
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            auxiliary_model: self.auxiliary_model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            reasoning_summary: self.reasoning_summary.clone(),
            revision: self.revision.clone(),
            thinking: self.thinking,
            num_ctx: self.num_ctx,
        }
    }

    fn apply_provider_state(&mut self, state: ProviderConfigState) {
        self.runtime_api_key = None;
        self.api_key = state.api_key;
        self.base_url = state.base_url;
        self.model = state.model;
        self.auxiliary_model = state.auxiliary_model;
        self.reasoning_effort = state.reasoning_effort;
        self.reasoning_summary = state.reasoning_summary;
        self.revision = state.revision;
        self.thinking = state.thinking;
        self.num_ctx = state.num_ctx;
    }

    fn apply_openai_profile(&mut self, profile: OpenAiEndpointProfile) {
        self.runtime_api_key = None;
        self.api_key = profile.api_key;
        self.base_url = profile.base_url;
        self.model = profile.model;
        self.auxiliary_model = profile.auxiliary_model;
        self.reasoning_effort = profile.reasoning_effort;
        self.reasoning_summary = profile.reasoning_summary;
        self.revision = profile.revision;
        self.thinking = None;
        self.num_ctx = None;
    }

    fn reset_provider_scoped_fields(&mut self) {
        self.runtime_api_key = None;
        self.api_key = None;
        self.base_url = None;
        self.model = None;
        self.auxiliary_model = None;
        self.reasoning_effort = None;
        self.reasoning_summary = Some(DEFAULT_REASONING_SUMMARY.to_string());
        self.revision = None;
        self.thinking = None;
        self.num_ctx = None;
    }

    fn sync_active_openai_profile(&mut self) {
        let profile_id = self.ensure_active_openai_profile_id();
        let mut profile = self
            .openai_profiles
            .get(&profile_id)
            .cloned()
            .unwrap_or_else(|| {
                self.default_openai_profile(
                    &profile_id,
                    OpenAiEndpointKind::Custom.label(),
                    OpenAiEndpointKind::Custom,
                )
            });
        profile.id = profile_id.clone();
        profile.api_key = self.api_key.clone();
        profile.base_url = self.base_url.clone();
        profile.model = self.model.clone();
        profile.auxiliary_model = self.auxiliary_model.clone();
        profile.reasoning_effort = self.reasoning_effort.clone();
        profile.reasoning_summary = self.reasoning_summary.clone();
        profile.revision = self.revision.clone();
        self.openai_profiles.insert(profile_id, profile);
    }

    fn ensure_active_openai_profile_id(&mut self) -> String {
        if let Some(existing) = self.active_openai_profile_id() {
            return existing.to_string();
        }
        let id = OpenAiEndpointKind::Custom.default_profile_id().to_string();
        self.active_openai_profile_id = Some(id.clone());
        id
    }

    fn default_openai_profile(
        &self,
        profile_id: &str,
        label: &str,
        kind: OpenAiEndpointKind,
    ) -> OpenAiEndpointProfile {
        OpenAiEndpointProfile {
            id: profile_id.to_string(),
            label: label.to_string(),
            kind,
            api_key: None,
            base_url: Some(kind.default_base_url().to_string()),
            model: Some(kind.default_model().to_string()),
            auxiliary_model: None,
            reasoning_effort: None,
            reasoning_summary: Some(DEFAULT_REASONING_SUMMARY.to_string()),
            revision: None,
        }
    }

    fn profile_for_kind_or_default(&self, kind: OpenAiEndpointKind) -> OpenAiEndpointProfile {
        self.openai_profiles
            .get(kind.default_profile_id())
            .cloned()
            .unwrap_or_else(|| {
                self.default_openai_profile(kind.default_profile_id(), kind.label(), kind)
            })
    }

    fn migrate_legacy_openai_profiles(&mut self) {
        let mut migrated_profiles = BTreeMap::new();
        let mut active_profile_id = self.active_openai_profile_id.clone();
        let current_provider = self.provider.clone();
        let mut should_apply_active_profile = false;
        let mut should_switch_provider = false;

        for legacy_provider in [
            "openai-compatible",
            "deepseek",
            "kimi",
            "kimi-coding",
            "openrouter",
        ] {
            let Some(kind) = OpenAiEndpointKind::from_legacy_provider(legacy_provider) else {
                continue;
            };
            let profile_id = kind.default_profile_id().to_string();
            let label = kind.label().to_string();

            if let Some(state) = self.provider_states.remove(legacy_provider) {
                migrated_profiles.insert(
                    profile_id.clone(),
                    OpenAiEndpointProfile {
                        id: profile_id.clone(),
                        label: label.clone(),
                        kind,
                        api_key: state.api_key,
                        base_url: normalize_optional_string(state.base_url)
                            .or_else(|| Some(kind.default_base_url().to_string())),
                        model: normalize_optional_string(state.model)
                            .or_else(|| Some(kind.default_model().to_string())),
                        auxiliary_model: normalize_optional_string(state.auxiliary_model),
                        reasoning_effort: normalize_optional_string(state.reasoning_effort),
                        reasoning_summary: normalize_reasoning_summary(state.reasoning_summary)
                            .or_else(|| Some(DEFAULT_REASONING_SUMMARY.to_string())),
                        revision: normalize_optional_string(state.revision),
                    },
                );
            }

            if current_provider == legacy_provider {
                should_apply_active_profile = true;
                should_switch_provider = legacy_provider != "openai-compatible";
                let existing_active_profile = if legacy_provider == "openai-compatible" {
                    self.active_openai_profile().cloned()
                } else {
                    None
                };
                if legacy_provider != "openai-compatible"
                    || (active_profile_id.is_none() && existing_active_profile.is_none())
                {
                    active_profile_id = Some(profile_id.clone());
                }
                let (target_profile_id, target_kind, target_label) =
                    if let Some(profile) = existing_active_profile {
                        (profile.id, profile.kind, profile.label)
                    } else {
                        (
                            active_profile_id
                                .clone()
                                .unwrap_or_else(|| profile_id.clone()),
                            kind,
                            label,
                        )
                    };
                migrated_profiles.insert(
                    target_profile_id.clone(),
                    OpenAiEndpointProfile {
                        id: target_profile_id,
                        label: target_label,
                        kind: target_kind,
                        api_key: self.api_key.clone(),
                        base_url: normalize_optional_string(self.base_url.clone())
                            .or_else(|| Some(target_kind.default_base_url().to_string())),
                        model: normalize_optional_string(self.model.clone())
                            .or_else(|| Some(target_kind.default_model().to_string())),
                        auxiliary_model: normalize_optional_string(self.auxiliary_model.clone()),
                        reasoning_effort: normalize_optional_string(self.reasoning_effort.clone()),
                        reasoning_summary: normalize_reasoning_summary(
                            self.reasoning_summary.clone(),
                        )
                        .or_else(|| Some(DEFAULT_REASONING_SUMMARY.to_string())),
                        revision: normalize_optional_string(self.revision.clone()),
                    },
                );
            }
        }

        if !migrated_profiles.is_empty() {
            self.openai_profiles.extend(migrated_profiles);
        }
        if should_switch_provider {
            self.provider = "openai-compatible".to_string();
        }
        if should_apply_active_profile {
            self.active_openai_profile_id = active_profile_id;
            if let Some(profile) = self.active_openai_profile().cloned() {
                self.apply_openai_profile(profile);
            }
        }
    }
}
