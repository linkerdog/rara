use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::provider_json::{expand, merge, parse_document};
use crate::provider_presets::provider_preset;
use crate::provider_registry::{ProviderDocument, ProviderRegistry, split_model_reference};
use crate::{ConfigManager, RaraConfig};

#[derive(Default)]
pub struct ProviderSelectionOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<SecretString>,
}

impl ConfigManager {
    pub fn load_for_project(&self, directory: &Path) -> Result<RaraConfig> {
        self.load_for_project_with_env(directory, &|key| std::env::var(key).ok())
    }

    pub fn load_for_project_with_overrides(
        &self,
        directory: &Path,
        overrides: &ProviderSelectionOverrides,
    ) -> Result<RaraConfig> {
        self.load_provider_inputs(directory, &|key| std::env::var(key).ok(), overrides)
    }

    pub fn load_for_project_with_env(
        &self,
        directory: &Path,
        read_env: &impl Fn(&str) -> Option<String>,
    ) -> Result<RaraConfig> {
        self.load_provider_inputs(directory, read_env, &ProviderSelectionOverrides::default())
    }

    pub(crate) fn load_provider_inputs(
        &self,
        directory: &Path,
        read_env: &impl Fn(&str) -> Option<String>,
        overrides: &ProviderSelectionOverrides,
    ) -> Result<RaraConfig> {
        let mut config = self.load()?;
        let mut paths = Vec::new();
        let global = self
            .path
            .parent()
            .context("Configuration root is missing")?;
        append_documents(&mut paths, global);
        if let Some(path) = read_env("RARA_CONFIG") {
            paths.push((directory.join(PathBuf::from(path)), true));
        }
        let mut ancestors = Vec::new();
        for ancestor in directory.ancestors() {
            ancestors.push(ancestor);
            if ancestor.join(".git").exists() {
                break;
            }
        }
        for ancestor in ancestors.into_iter().rev() {
            if ancestor != global {
                append_documents(&mut paths, ancestor);
            }
        }
        let mut merged = Value::Object(Default::default());
        let mut found = false;
        for (path, required) in paths {
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("Cannot read {}", path.display()));
                }
            };
            let mut document = parse_document(&content)
                .with_context(|| format!("Cannot parse {}", path.display()))?;
            expand(&mut document, path.parent().unwrap_or(directory), read_env)?;
            merge(&mut merged, document);
            found = true;
        }
        if let Some(content) = read_env("RARA_CONFIG_CONTENT") {
            let mut document = parse_document(&content).context("Invalid RARA_CONFIG_CONTENT")?;
            expand(&mut document, directory, read_env)?;
            merge(&mut merged, document);
            found = true;
        }
        if !found {
            return Ok(config);
        }
        let mut document: ProviderDocument = serde_json::from_value(merged)
            .map_err(|_| anyhow::anyhow!("Invalid provider configuration schema; check provider/models/options fields and their types"))?;
        resolve_providers(&mut document, read_env)?;
        for (provider, credential) in self.load_provider_auth()? {
            if let Some(definition) = document.provider.get_mut(&provider)
                && definition.options.api_key.is_none()
            {
                definition.options.api_key = Some(credential.key);
            }
        }
        config.provider_baseline = Some(Box::new(config.clone()));
        config.provider_registry = ProviderRegistry {
            document,
            selected: None,
        };
        let cli_selection = overrides.model.is_some() || overrides.provider.is_some();
        let explicit = if cli_selection {
            overrides
                .model
                .as_ref()
                .and_then(|model| {
                    if model.split_once('/').is_some_and(|(provider, _)| {
                        config
                            .provider_registry
                            .document
                            .provider
                            .contains_key(provider)
                    }) {
                        Some(model.clone())
                    } else {
                        let provider = overrides.provider.as_deref().unwrap_or(&config.provider);
                        config
                            .provider_registry
                            .document
                            .provider
                            .contains_key(provider)
                            .then(|| format!("{provider}/{model}"))
                    }
                })
                .or_else(|| {
                    overrides.provider.as_ref().and_then(|provider| {
                        config
                            .provider_registry
                            .document
                            .provider
                            .get(provider)?
                            .models
                            .keys()
                            .find(|model| config.provider_registry.model_allowed(provider, model))
                            .map(|model| format!("{provider}/{model}"))
                    })
                })
        } else {
            config.provider_registry.document.model.clone()
        };
        if cli_selection
            && explicit.is_none()
            && config.provider == "mock"
            && overrides.provider.is_none()
        {
            bail!("CLI model must select a configured provider/model");
        }
        if let Some(key) = overrides.api_key.as_ref()
            && let Some(reference) = &explicit
        {
            let (provider, _) = split_model_reference(reference)?;
            if let Some(definition) = config.provider_registry.document.provider.get_mut(provider) {
                definition.options.api_key = Some(key.clone());
                definition.credential_override = true;
            }
        }
        let recent = if explicit.is_none() && !cli_selection {
            match fs::read_to_string(global.join("model-selection.json")) {
                Ok(content) => Some(
                    serde_json::from_str::<String>(&content)
                        .context("Invalid saved model selection")?,
                ),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => return Err(err).context("Cannot read saved model selection"),
            }
        } else {
            None
        };
        if let Some(reference) = explicit.as_ref().or(recent.as_ref()) {
            let (provider, model) = split_model_reference(reference)?;
            if explicit.is_some() && !config.provider_registry.model_allowed(provider, model) {
                bail!("Model '{reference}' is unknown or disabled in provider configuration");
            }
            if explicit.is_some()
                || (config.provider_registry.model_allowed(provider, model)
                    && config.provider_registry.available(provider))
            {
                config.select_registry_model(provider, model)?;
            }
        }
        if !cli_selection
            && config.provider_registry.selected.is_none()
            && config.provider == "mock"
        {
            let first =
                config
                    .provider_registry
                    .document
                    .provider
                    .iter()
                    .find_map(|(id, definition)| {
                        definition
                            .models
                            .keys()
                            .find(|model| {
                                config.provider_registry.model_allowed(id, model)
                                    && config.provider_registry.available(id)
                            })
                            .map(|model| (id.clone(), model.clone()))
                    });
            if let Some((provider, model)) = first {
                config.select_registry_model(&provider, &model)?;
            }
        }
        Ok(config)
    }
}

fn append_documents(paths: &mut Vec<(PathBuf, bool)>, directory: &Path) {
    paths.push((directory.join("rara.json"), false));
    paths.push((directory.join("rara.jsonc"), false));
}

fn resolve_providers(
    document: &mut ProviderDocument,
    read_env: &impl Fn(&str) -> Option<String>,
) -> Result<()> {
    for (id, definition) in &mut document.provider {
        if id.is_empty() || id.contains('/') {
            bail!("Provider IDs must be non-empty and cannot contain '/'");
        }
        if definition
            .npm
            .as_deref()
            .is_some_and(|npm| npm != "@ai-sdk/openai-compatible")
        {
            bail!(
                "Provider '{id}' requests an unsupported transport; this registry supports @ai-sdk/openai-compatible only"
            );
        }
        if let Some(preset) = provider_preset(id) {
            definition
                .name
                .get_or_insert_with(|| preset.name.to_string());
            definition
                .options
                .base_url
                .get_or_insert_with(|| preset.base_url.to_string());
            if definition.env.is_empty() {
                definition.env.push(preset.env.to_string());
            }
        }
        let base = definition
            .options
            .base_url
            .as_deref()
            .with_context(|| format!("Provider '{id}' requires options.baseURL"))?;
        if !(base.starts_with("http://") || base.starts_with("https://"))
            || base.contains('?')
            || base.contains('#')
        {
            bail!("Provider '{id}' requires an http(s) API root without query or fragment");
        }
        if definition.options.api_key.is_none() {
            definition.options.api_key = definition
                .env
                .iter()
                .find_map(|key| read_env(key).filter(|value| !value.trim().is_empty()))
                .map(SecretString::from);
        }
        if definition
            .options
            .api_key
            .as_ref()
            .is_some_and(|key| key.expose_secret().trim().is_empty())
        {
            definition.options.api_key = None;
        }
        definition.credential_override = definition.options.api_key.is_some();
        for (model, entry) in &definition.models {
            if model.is_empty() || entry.id.as_ref().is_some_and(|id| id.is_empty()) {
                bail!("Provider '{id}' has an empty model ID");
            }
            if entry.limit.context == Some(0) || entry.limit.output == Some(0) {
                bail!("Model '{id}/{model}' token limits must be positive");
            }
            if let (Some(context), Some(output)) = (entry.limit.context, entry.limit.output)
                && output >= context
            {
                bail!("Model '{id}/{model}' output limit must be smaller than its context window");
            }
            if entry
                .options
                .temperature
                .is_some_and(|value| !(0.0..=2.0).contains(&value))
                || entry
                    .options
                    .top_p
                    .is_some_and(|value| !(0.0..=1.0).contains(&value))
            {
                bail!("Model '{id}/{model}' has invalid sampling options");
            }
        }
    }
    Ok(())
}
