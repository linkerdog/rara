use std::collections::HashSet;

use anyhow::{Result, anyhow};
use rara_config::DEFAULT_KIMI_BASE_URL;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::redaction::{redact_known_secret, sanitize_url_for_display};
use crate::{ModelCatalogEntry, ModelCatalogRequest};

const MODELS_TIMEOUT_SECS: u64 = 15;

/// Model name → context window tokens (for budget calculation).
/// Also serves as the fallback model list when the API is unavailable.
pub const MODEL_WINDOWS: &[(&str, u32)] = &[
    ("kimi-k3", 1_048_576),
    ("kimi-k2.6", 262_144),
    ("kimi-k2.5", 262_144),
    ("kimi-k2-0905-preview", 262_144),
    ("kimi-k2-turbo-preview", 262_144),
    ("kimi-k2-thinking", 262_144),
    ("kimi-k2-thinking-turbo", 262_144),
];

pub fn fallback_models() -> Vec<String> {
    MODEL_WINDOWS
        .iter()
        .map(|(model, _)| (*model).to_string())
        .collect()
}

pub fn fallback_catalog() -> Vec<ModelCatalogEntry> {
    MODEL_WINDOWS
        .iter()
        .map(|(id, context_window)| ModelCatalogEntry {
            id: (*id).to_string(),
            context_window: Some(*context_window),
        })
        .collect()
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(alias = "context_window", alias = "max_context_length")]
    context_length: Option<u32>,
}

pub fn models_url(base_url: Option<&str>) -> String {
    let base_url = base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_KIMI_BASE_URL)
        .trim_end_matches('/');
    format!("{base_url}/models")
}

pub fn parse_models(body: &str) -> Result<Vec<ModelCatalogEntry>> {
    let response: ModelsResponse = serde_json::from_str(body)?;
    let mut seen = HashSet::new();
    let models = response
        .data
        .into_iter()
        .filter_map(|model| {
            let id = model.id.trim().to_string();
            (!id.is_empty() && seen.insert(id.clone())).then_some(ModelCatalogEntry {
                context_window: model.context_length.or_else(|| {
                    MODEL_WINDOWS
                        .iter()
                        .find(|(name, _)| *name == id)
                        .map(|(_, window)| *window)
                }),
                id,
            })
        })
        .collect::<Vec<_>>();
    Ok(models)
}

pub async fn load_models(request: ModelCatalogRequest<'_>) -> Result<Vec<ModelCatalogEntry>> {
    let api_key = request
        .api_key
        .map(SecretString::expose_secret)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("Kimi API key is required to list models"))?;
    let url = models_url(request.base_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(MODELS_TIMEOUT_SECS))
        .build()?;
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .bearer_auth(api_key)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(anyhow!(
            "Kimi model list request failed at {}: {}",
            sanitize_url_for_display(&url),
            redact_known_secret(&body, api_key)
        ));
    }
    parse_models(&body)
}

#[cfg(test)]
mod tests {
    use super::{models_url, parse_models};

    #[test]
    fn kimi_models_url_uses_root_models_endpoint() {
        assert_eq!(
            models_url(Some("https://api.moonshot.ai/v1")),
            "https://api.moonshot.ai/v1/models"
        );
        assert_eq!(
            models_url(Some("https://api.moonshot.ai")),
            "https://api.moonshot.ai/models"
        );
    }

    #[test]
    fn parses_kimi_models_in_provider_order() {
        let models = parse_models(
            r#"{
                "object": "list",
                "data": [
                    {"id": "kimi-k2.6", "object": "model", "context_length": 262144},
                    {"id": "kimi-k2.5", "object": "model"},
                    {"id": "kimi-k2.6", "object": "model"},
                    {"id": " ", "object": "model"}
                ]
            }"#,
        )
        .expect("parse models");

        assert_eq!(
            models,
            vec![
                super::ModelCatalogEntry {
                    id: "kimi-k2.6".to_string(),
                    context_window: Some(262_144),
                },
                super::ModelCatalogEntry {
                    id: "kimi-k2.5".to_string(),
                    context_window: Some(262_144),
                },
            ]
        );
    }

    #[test]
    fn fallback_models_include_kimi_k3_first() {
        let models = super::fallback_models();

        assert_eq!(models.first().map(String::as_str), Some("kimi-k3"));
        assert!(models.iter().any(|model| model == "kimi-k2.6"));
    }
}
