use anyhow::{Result, anyhow};
use rara_config::DEFAULT_KIMI_BASE_URL;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::ModelCatalogRequest;
use crate::redaction::{redact_known_secret, sanitize_url_for_display};

const MODELS_TIMEOUT_SECS: u64 = 15;

/// Model name → context window tokens (for budget calculation).
/// Also serves as the fallback model list when the API is unavailable.
pub const MODEL_WINDOWS: &[(&str, u32)] = &[
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

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

pub fn models_url(base_url: Option<&str>) -> String {
    let base_url = base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_KIMI_BASE_URL)
        .trim_end_matches('/');
    let root = base_url.strip_suffix("/v1").unwrap_or(base_url);
    format!("{root}/models")
}

pub fn parse_models(body: &str) -> Result<Vec<String>> {
    let response: ModelsResponse = serde_json::from_str(body)?;
    let models = response
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    Ok(models)
}

pub async fn load_models(request: ModelCatalogRequest<'_>) -> Result<Vec<String>> {
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
            models_url(Some("https://api.moonshot.cn/v1")),
            "https://api.moonshot.cn/models"
        );
        assert_eq!(
            models_url(Some("https://api.moonshot.cn")),
            "https://api.moonshot.cn/models"
        );
    }

    #[test]
    fn parses_kimi_models_in_provider_order() {
        let models = parse_models(
            r#"{
                "object": "list",
                "data": [
                    {"id": "kimi-k2.6", "object": "model"},
                    {"id": "kimi-k2.5", "object": "model"},
                    {"id": "kimi-k2.5", "object": "model"},
                    {"id": " ", "object": "model"}
                ]
            }"#,
        )
        .expect("parse models");

        assert_eq!(models, vec!["kimi-k2.6", "kimi-k2.5", "kimi-k2.5"]);
    }
}
