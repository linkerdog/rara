use super::maintenance::request_maintenance;
use crate::agent::Agent;
use crate::oauth::OAuthManager;
use crate::tui::provider_flow::{
    should_open_codex_auth_guide, sync_codex_credential_from_auth_store,
};
use crate::tui::runtime_port::{RuntimeClientPort, RuntimeMaintenanceCommand};
use crate::tui::state::{
    ApiKeyTarget, ListPickerKind, Overlay, ProviderFamily, TuiApp, UnifiedModelPreset,
};

pub(super) async fn apply_model_selection(
    preset: UnifiedModelPreset,
    app: &mut TuiApp,
    agent_slot: &Option<Agent>,
    oauth_manager: &OAuthManager,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<()> {
    let Some(index) = app
        .all_unified_model_presets()
        .iter()
        .position(|candidate| {
            candidate.family == preset.family
                && candidate.provider_id == preset.provider_id
                && candidate.model_id == preset.model_id
        })
    else {
        app.push_notice("The selected model is no longer available. Reopen /model.");
        return Ok(());
    };
    app.select_unified_model(index);

    match preset.family {
        ProviderFamily::Codex => {
            sync_codex_credential_from_auth_store(app, oauth_manager)?;
            if should_open_codex_auth_guide(app, oauth_manager) {
                app.open_overlay(Overlay::ListPicker(ListPickerKind::AuthMode));
            } else if app.selected_codex_reasoning_options().len() <= 1 {
                app.apply_selected_codex_reasoning_effort();
                request_maintenance(
                    app,
                    agent_slot,
                    runtime_port,
                    RuntimeMaintenanceCommand::Rebuild,
                )
                .await?;
            } else {
                app.open_overlay(Overlay::ListPicker(ListPickerKind::ReasoningEffort));
            }
        }
        ProviderFamily::OpenAiCompatible if app.openai_profile_needs_setup() => {
            app.begin_active_openai_profile_setup();
        }
        ProviderFamily::DeepSeek if !app.config.has_api_key() => {
            app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek));
        }
        ProviderFamily::Kimi if !app.config.has_api_key() => {
            app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Kimi));
        }
        ProviderFamily::KimiCoding if !app.config.has_api_key() => {
            app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::KimiCoding));
        }
        ProviderFamily::Gemini if !app.config.has_api_key() => {
            app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Gemini));
        }
        ProviderFamily::CandleLocal => {
            app.push_notice("Local models (alpha) are for preview only.");
            app.dismiss_overlay();
        }
        ProviderFamily::OpenAiCompatible
        | ProviderFamily::DeepSeek
        | ProviderFamily::Kimi
        | ProviderFamily::KimiCoding
        | ProviderFamily::Gemini
        | ProviderFamily::Ollama
        | ProviderFamily::Bedrock => {
            if preset.family == ProviderFamily::DeepSeek {
                app.config.reasoning_effort = Some("max".to_string());
            }
            request_maintenance(
                app,
                agent_slot,
                runtime_port,
                RuntimeMaintenanceCommand::Rebuild,
            )
            .await?;
        }
    }
    Ok(())
}
