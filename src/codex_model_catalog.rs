use std::path::Path;

use anyhow::Result;
use codex_login::{AuthCredentialsStoreMode, AuthManager};
use codex_models_manager::bundled_models_response;
use codex_models_manager::manager::{ModelsManager, RefreshStrategy, StaticModelsManager};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CodexReasoningOption {
    pub value: String,
    pub label: String,
    pub description: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CodexModelOption {
    pub id: String,
    pub model: String,
    pub label: String,
    pub description: String,
    pub reasoning_options: Vec<CodexReasoningOption>,
    pub default_reasoning_effort: Option<String>,
    pub is_default: bool,
}

pub async fn load_codex_model_catalog(
    codex_home: &Path,
    refresh_strategy: RefreshStrategy,
) -> Result<Vec<CodexModelOption>> {
    let auth_manager = AuthManager::shared(
        codex_home.to_path_buf(),
        false,
        AuthCredentialsStoreMode::File,
        None,
    )
    .await;
    let manager = StaticModelsManager::new(Some(auth_manager), bundled_models_response()?);
    let mut models = manager.list_models(refresh_strategy).await;
    if models.iter().any(|model| model.show_in_picker) {
        models.retain(|model| model.show_in_picker);
    }

    Ok(models
        .into_iter()
        .map(|preset| {
            let default_reasoning_effort = Some(preset.default_reasoning_effort.to_string());
            let reasoning_options = preset
                .supported_reasoning_efforts
                .into_iter()
                .map(|option| {
                    let value = option.effort.to_string();
                    CodexReasoningOption {
                        label: reasoning_effort_label(&value).to_string(),
                        is_default: default_reasoning_effort.as_deref() == Some(value.as_str()),
                        value,
                        description: option.description,
                    }
                })
                .collect::<Vec<_>>();

            CodexModelOption {
                id: preset.id,
                model: preset.model,
                label: preset.display_name,
                description: preset.description,
                reasoning_options,
                default_reasoning_effort,
                is_default: preset.is_default,
            }
        })
        .collect())
}

pub fn reasoning_effort_label(value: &str) -> &'static str {
    match value {
        "none" => "None",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        _ => "Custom",
    }
}
