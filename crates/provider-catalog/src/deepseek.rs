use anyhow::{Result, anyhow};
use rara_config::DEFAULT_DEEPSEEK_BASE_URL;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::ModelCatalogRequest;
use crate::redaction::{redact_known_secret, sanitize_url_for_display};

const MODELS_TIMEOUT_SECS: u64 = 15;

pub const FALLBACK_MODELS: [&str; 3] = ["deepseek-chat", "deepseek-reasoner", "deepseek-v4"];

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

pub fn fallback_models() -> Vec<String> {
    FALLBACK_MODELS
        .iter()
        .map(|model| (*model).to_string())
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
    fn parses_deepseek_models_in_provider_order() {
        let models = parse_models(
            r#"{
                "object": "list",
                "data": [
                    {"id": "deepseek-reasoner", "object": "model"},
                    {"id": "deepseek-chat", "object": "model"},
                    {"id": "deepseek-chat", "object": "model"},
                    {"id": " ", "object": "model"}
                ]
            }"#,
        )
        .expect("parse models");

        assert_eq!(
            models,
            vec!["deepseek-reasoner", "deepseek-chat", "deepseek-chat"]
        );
    }
}
