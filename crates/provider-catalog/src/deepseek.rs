use std::collections::HashSet;

use anyhow::{Result, anyhow};
use rara_config::DEFAULT_DEEPSEEK_BASE_URL;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::redaction::{redact_known_secret, sanitize_url_for_display};
use crate::{ModelCatalogEntry, ModelCatalogRequest};

const MODELS_TIMEOUT_SECS: u64 = 15;

/// Model name → context window tokens (for budget calculation).
///
/// This includes accepted compatibility aliases which must retain their correct
/// context budget, even though the picker only advertises current model IDs.
pub const MODEL_WINDOWS: &[(&str, u32)] = &[
    ("deepseek-flash", 1_048_576),
    ("deepseek-v4-pro", 1_048_576),
    ("deepseek-v4-flash", 1_048_576),
    ("deepseek-v4-flash-vision-exp", 1_048_576),
];

/// Current model IDs documented by DeepSeek and shown when API discovery is
/// unavailable. Compatibility aliases remain usable through manual or saved
/// configuration but are deliberately not advertised here.
pub const FALLBACK_MODELS: [&str; 2] = ["deepseek-flash", "deepseek-v4-pro"];
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

pub fn fallback_models() -> Vec<String> {
    FALLBACK_MODELS
        .iter()
        .map(|model| (*model).to_string())
        .collect()
}

pub fn fallback_catalog() -> Vec<ModelCatalogEntry> {
    FALLBACK_MODELS
        .iter()
        .map(|id| ModelCatalogEntry {
            id: (*id).to_string(),
            context_window: MODEL_WINDOWS
                .iter()
                .find(|(known_id, _)| known_id == id)
                .map(|(_, window)| *window),
        })
        .collect()
}

pub fn models_url(base_url: Option<&str>) -> String {
    let base_url = base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DEEPSEEK_BASE_URL)
        .trim_end_matches('/');
    let root = base_url.strip_suffix("/v1").unwrap_or(base_url);
    format!("{root}/models")
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
        .ok_or_else(|| anyhow!("DeepSeek API key is required to list models"))?;
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
            "DeepSeek model list request failed at {}: {}",
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
    fn deepseek_models_url_uses_root_models_endpoint() {
        assert_eq!(
            models_url(Some("https://api.deepseek.com/v1")),
            "https://api.deepseek.com/models"
        );
        assert_eq!(
            models_url(Some("https://api.deepseek.com")),
            "https://api.deepseek.com/models"
        );
    }

    #[test]
    fn parses_current_deepseek_models_with_known_context_windows() {
        let models = parse_models(
            r#"{
                "object": "list",
                "data": [
                    {"id": "deepseek-flash", "object": "model"},
                    {"id": "deepseek-v4-pro", "object": "model"},
                    {"id": "deepseek-flash", "object": "model"},
                    {"id": " ", "object": "model"}
                ]
            }"#,
        )
        .expect("parse models");

        assert_eq!(
            models,
            vec![
                super::ModelCatalogEntry {
                    id: "deepseek-flash".to_string(),
                    context_window: Some(1_048_576),
                },
                super::ModelCatalogEntry {
                    id: "deepseek-v4-pro".to_string(),
                    context_window: Some(1_048_576),
                },
            ]
        );
    }

    #[test]
    fn fallback_catalog_lists_only_current_deepseek_model_ids() {
        assert_eq!(
            super::fallback_models(),
            vec!["deepseek-flash".to_string(), "deepseek-v4-pro".to_string()]
        );
        assert_eq!(
            super::fallback_catalog(),
            vec![
                super::ModelCatalogEntry {
                    id: "deepseek-flash".to_string(),
                    context_window: Some(1_048_576),
                },
                super::ModelCatalogEntry {
                    id: "deepseek-v4-pro".to_string(),
                    context_window: Some(1_048_576),
                },
            ]
        );
    }
}
