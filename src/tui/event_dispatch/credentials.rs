use rara_provider_catalog::ModelCatalogProvider;

use super::maintenance::request_maintenance;
use crate::agent::Agent;
use crate::config::DEFAULT_CODEX_BASE_URL;
use crate::oauth::SavedCodexAuthMode;
use crate::tui::runtime_port::{RuntimeClientPort, RuntimeMaintenanceCommand};
use crate::tui::state::{ApiKeyTarget, NoticeLevel, Overlay, TuiApp};

pub(super) async fn save_api_key(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<()> {
    let Some(Overlay::ApiKeyEditor(target)) = app.overlay else {
        app.push_notice(NoticeLevel::Warning, "API key editor is no longer active.");
        return Ok(());
    };
    let value = app.api_key_input.trim().to_string();
    if target == ApiKeyTarget::Registry {
        if app.is_busy() {
            app.push_notice(
                NoticeLevel::Info,
                "Wait for the current task before saving the API key.",
            );
            return Ok(());
        }
        if value.is_empty() {
            app.push_notice(
                NoticeLevel::Info,
                "Enter an API key or press Esc to go back.",
            );
            return Ok(());
        }
        let provider = app
            .registry_credential_target
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Provider credential target is missing"))?;
        app.config_manager
            .save_registry_api_key(&provider, secrecy::SecretString::from(value.clone()))?;
        let mut overridden = false;
        if let Some(definition) = app
            .config
            .provider_registry
            .document
            .provider
            .get_mut(&provider)
        {
            overridden = definition.credential_override;
            if !overridden {
                definition.options.api_key = Some(secrecy::SecretString::from(value));
            }
        }
        app.api_key_input.clear();
        app.registry_credential_target = None;
        app.dismiss_overlay();
        app.push_notice(
            NoticeLevel::Info,
            format!("Saved {provider} API key. Select a model with /model."),
        );
        if overridden {
            app.push_notice(
                NoticeLevel::Warning,
                "The configured or environment API key takes precedence over the saved credential.",
            );
        }
        return Ok(());
    }
    if app.is_busy() {
        app.push_notice(
            NoticeLevel::Info,
            "Wait for the current task before saving the API key.",
        );
    } else if value.is_empty() && target != ApiKeyTarget::OpenAiCompatible {
        app.push_notice(
            NoticeLevel::Info,
            format!(
                "Enter a {} API key or press Esc to go back.",
                target.label()
            ),
        );
    } else if value.is_empty() && app.openai_setup_keep_empty_api_key {
        app.push_notice(
            NoticeLevel::Info,
            "Kept existing API key for the current profile.",
        );
        app.advance_openai_profile_setup();
    } else if value.is_empty() {
        app.config.clear_api_key();
        if app.config.provider == "codex" {
            app.codex_auth_mode = None;
        }
        app.config_manager.save(&app.config)?;
        app.push_notice(
            NoticeLevel::Info,
            "Cleared API key for the current provider.",
        );
        if app.openai_setup_steps.is_empty() {
            app.dismiss_overlay();
        } else {
            app.advance_openai_profile_setup();
        }
    } else {
        let codex_is_active = app.config.provider == "codex";
        match target {
            ApiKeyTarget::Registry => {
                anyhow::bail!("registry credential reached the legacy key editor");
            }
            ApiKeyTarget::Codex => app.config.set_provider_api_key("codex", value),
            ApiKeyTarget::DeepSeek => app.config.set_provider_api_key("deepseek", value),
            ApiKeyTarget::Kimi => app.config.set_provider_api_key("kimi", value),
            ApiKeyTarget::KimiCoding => app.config.set_provider_api_key("kimi-coding", value),
            ApiKeyTarget::OpenAiCompatible => app.config.set_api_key(value),
            ApiKeyTarget::Gemini => app.config.set_provider_api_key("gemini", value),
        }
        if target == ApiKeyTarget::Codex {
            app.codex_auth_mode = Some(SavedCodexAuthMode::ApiKey);
            if codex_is_active {
                app.config
                    .apply_codex_defaults_for_base_url(DEFAULT_CODEX_BASE_URL);
            }
        } else if target == ApiKeyTarget::KimiCoding {
            app.config.set_provider("kimi-coding");
        }
        app.config_manager.save(&app.config)?;
        if target == ApiKeyTarget::Codex && codex_is_active {
            app.push_notice(
                NoticeLevel::Info,
                "Saved Codex API key. Rebuilding backend.",
            );
            app.dismiss_overlay();
            request_maintenance(
                app,
                agent_slot,
                runtime_port,
                RuntimeMaintenanceCommand::Rebuild,
            )
            .await?;
        } else if target == ApiKeyTarget::DeepSeek {
            app.push_notice(NoticeLevel::Info, "Saved DeepSeek API key. Loading models.");
            app.dismiss_overlay();
            request_maintenance(
                app,
                agent_slot,
                runtime_port,
                RuntimeMaintenanceCommand::RefreshModelCatalog(ModelCatalogProvider::DeepSeek),
            )
            .await?;
        } else if target == ApiKeyTarget::Kimi {
            app.push_notice(
                NoticeLevel::Info,
                "Saved Moonshot AI API key. Loading models.",
            );
            app.dismiss_overlay();
            request_maintenance(
                app,
                agent_slot,
                runtime_port,
                RuntimeMaintenanceCommand::RefreshModelCatalog(ModelCatalogProvider::Kimi),
            )
            .await?;
        } else if target == ApiKeyTarget::KimiCoding {
            app.push_notice(
                NoticeLevel::Info,
                "Saved Kimi For Coding API key. Rebuilding backend.",
            );
            app.dismiss_overlay();
            request_maintenance(
                app,
                agent_slot,
                runtime_port,
                RuntimeMaintenanceCommand::Rebuild,
            )
            .await?;
        } else {
            app.push_notice(
                NoticeLevel::Info,
                match target {
                    ApiKeyTarget::Registry => "Saved provider API key.",
                    ApiKeyTarget::Codex => "Saved Codex API key.",
                    ApiKeyTarget::DeepSeek => "Saved DeepSeek API key.",
                    ApiKeyTarget::Kimi => "Saved Moonshot AI API key.",
                    ApiKeyTarget::KimiCoding => "Saved Kimi For Coding API key.",
                    ApiKeyTarget::OpenAiCompatible => {
                        "Saved API key for the current endpoint profile."
                    }
                    ApiKeyTarget::Gemini => "Saved Gemini API key.",
                },
            );
            if app.openai_setup_steps.is_empty() {
                app.dismiss_overlay();
            } else {
                app.advance_openai_profile_setup();
            }
        }
    }
    Ok(())
}
