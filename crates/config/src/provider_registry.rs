use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::RaraConfig;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDocument {
    #[serde(rename = "$schema")]
    pub schema: Option<String>,
    pub model: Option<String>,
    pub small_model: Option<String>,
    #[serde(default)]
    pub provider: BTreeMap<String, ProviderDefinition>,
    pub enabled_providers: Option<Vec<String>>,
    #[serde(default)]
    pub disabled_providers: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDefinition {
    #[serde(skip)]
    pub credential_override: bool,
    pub name: Option<String>,
    pub npm: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub options: ProviderOptions,
    #[serde(default)]
    pub models: BTreeMap<String, ProviderModel>,
    pub whitelist: Option<Vec<String>>,
    #[serde(default)]
    pub blacklist: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderOptions {
    #[serde(rename = "baseURL")]
    pub base_url: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::secrets::deserialize_secret_option"
    )]
    pub api_key: Option<SecretString>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderModel {
    pub id: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub limit: ModelLimits,
    #[serde(default)]
    pub options: ModelOptions,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLimits {
    pub context: Option<u32>,
    pub output: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelOptions {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderRegistry {
    pub document: ProviderDocument,
    pub selected: Option<(String, String)>,
}

impl ProviderRegistry {
    pub fn provider_allowed(&self, id: &str) -> bool {
        !self
            .document
            .disabled_providers
            .iter()
            .any(|value| value == id)
            && self
                .document
                .enabled_providers
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|value| value == id))
    }

    pub fn model_allowed(&self, provider: &str, model: &str) -> bool {
        self.provider_allowed(provider)
            && self
                .document
                .provider
                .get(provider)
                .is_some_and(|definition| {
                    definition.models.contains_key(model)
                        && !definition.blacklist.iter().any(|value| value == model)
                        && definition
                            .whitelist
                            .as_ref()
                            .is_none_or(|ids| ids.iter().any(|value| value == model))
                })
    }

    pub fn available(&self, provider: &str) -> bool {
        self.provider_allowed(provider)
            && self
                .document
                .provider
                .get(provider)
                .is_some_and(|definition| {
                    definition
                        .options
                        .api_key
                        .as_ref()
                        .is_some_and(|key| !key.expose_secret().trim().is_empty())
                        || crate::provider_presets::provider_preset(provider).is_none()
                })
    }
}

impl RaraConfig {
    pub fn select_registry_model(&mut self, provider: &str, model: &str) -> Result<()> {
        if !self.provider_registry.model_allowed(provider, model) {
            bail!("Model '{provider}/{model}' is unknown or disabled in provider configuration");
        }
        if !self.provider_registry.available(provider) {
            bail!(
                "Provider '{provider}' needs an API key in options.apiKey or its credential environment variable"
            );
        }
        let definition = &self.provider_registry.document.provider[provider];
        let entry = &definition.models[model];
        let auxiliary_model = self.provider_registry.document.small_model.as_deref().map(|reference| {
            let (small_provider, small_model) = split_model_reference(reference)?;
            if small_provider != provider {
                bail!("Cross-provider small_model is not supported yet; configure a model from '{provider}'");
            }
            if !self.provider_registry.model_allowed(provider, small_model) {
                bail!("small_model '{reference}' is unknown or disabled");
            }
            let entry = &definition.models[small_model];
            Ok(entry.id.clone().unwrap_or_else(|| small_model.to_string()))
        }).transpose()?;
        self.provider = provider.to_string();
        self.model = Some(entry.id.clone().unwrap_or_else(|| model.to_string()));
        self.api_key = None;
        self.runtime_api_key = definition.options.api_key.clone();
        self.base_url = definition.options.base_url.clone();
        self.auxiliary_model = auxiliary_model;
        self.reasoning_effort = entry.options.reasoning_effort.clone();
        self.reasoning_summary = None;
        self.revision = None;
        self.thinking = None;
        self.num_ctx = None;
        self.active_openai_profile_id = None;
        self.provider_registry.selected = Some((provider.to_string(), model.to_string()));
        Ok(())
    }

    pub fn selected_registry_model(&self) -> Option<&ProviderModel> {
        let (provider, model) = self.provider_registry.selected.as_ref()?;
        if provider != &self.provider {
            return None;
        }
        let entry = self
            .provider_registry
            .document
            .provider
            .get(provider)?
            .models
            .get(model)?;
        (self.model.as_deref() == Some(entry.id.as_deref().unwrap_or(model))).then_some(entry)
    }

    pub fn resolve_registry_model_reference(&mut self) -> Result<()> {
        if self.selected_registry_model().is_some() {
            return Ok(());
        }
        let Some(reference) = self.model.clone() else {
            return Ok(());
        };
        if let Some((provider, model)) = reference.split_once('/')
            && self
                .provider_registry
                .document
                .provider
                .contains_key(provider)
        {
            self.select_registry_model(provider, model)?;
        } else if let Some(definition) =
            self.provider_registry.document.provider.get(&self.provider)
        {
            let model = if definition.models.contains_key(&reference) {
                reference
            } else {
                let candidates: Vec<_> = definition
                    .models
                    .iter()
                    .filter(|(_, entry)| entry.id.as_deref() == Some(reference.as_str()))
                    .map(|(id, _)| id.clone())
                    .collect();
                match candidates.as_slice() {
                    [model] => model.clone(),
                    _ => bail!(
                        "Model '{reference}' is unknown or ambiguous; use provider/model with a configured model key"
                    ),
                }
            };
            let provider = self.provider.clone();
            self.select_registry_model(&provider, &model)?;
        }
        Ok(())
    }
}

pub fn split_model_reference(reference: &str) -> Result<(&str, &str)> {
    let (provider, model) = reference
        .split_once('/')
        .context("Model must use provider/model syntax")?;
    if provider.is_empty() || model.is_empty() {
        bail!("Model must use non-empty provider/model syntax");
    }
    Ok((provider, model))
}
