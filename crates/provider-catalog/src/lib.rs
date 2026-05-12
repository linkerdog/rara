pub mod deepseek;
pub mod kimi;
mod redaction;

use anyhow::Result;
use secrecy::SecretString;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelCatalogProvider {
    DeepSeek,
    Kimi,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ModelCatalogRequest<'a> {
    pub api_key: Option<&'a SecretString>,
    pub base_url: Option<&'a str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCatalog {
    pub provider: ModelCatalogProvider,
    pub models: Vec<String>,
}

pub fn fallback_models(provider: ModelCatalogProvider) -> Vec<String> {
    match provider {
        ModelCatalogProvider::DeepSeek => deepseek::fallback_models(),
        ModelCatalogProvider::Kimi => kimi::fallback_models(),
    }
}

pub async fn load_model_catalog(
    provider: ModelCatalogProvider,
    request: ModelCatalogRequest<'_>,
) -> Result<ModelCatalog> {
    let models = match provider {
        ModelCatalogProvider::DeepSeek => deepseek::load_models(request).await?,
        ModelCatalogProvider::Kimi => kimi::load_models(request).await?,
    };
    Ok(ModelCatalog { provider, models })
}
