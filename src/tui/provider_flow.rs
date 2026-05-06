use codex_models_manager::manager::RefreshStrategy;
use secrecy::{ExposeSecret, SecretString};

use super::state::{ListPickerKind, Overlay, ProviderFamily, TuiApp};
use crate::agent::Agent;
use crate::codex_model_catalog::load_codex_model_catalog;
use crate::config::{OpenAiEndpointKind, ensure_rara_home_dir};
use crate::google_oauth::GoogleOAuthManager;
use crate::oauth::OAuthManager;

pub(super) fn sync_codex_credential_from_auth_store(
    app: &mut TuiApp,
    oauth_manager: &OAuthManager,
) -> anyhow::Result<bool> {
    let saved_auth_mode = oauth_manager.saved_auth_mode()?;
    app.codex_auth_mode = saved_auth_mode;
    let codex_state = app.config.provider_states.get("codex");
    let has_ready_codex_state = codex_state
        .and_then(|state| state.api_key.as_ref())
        .is_some_and(|api_key| !api_key.expose_secret().trim().is_empty())
        && codex_state
            .and_then(|state| state.model.as_deref())
            .is_some_and(|model| !crate::config::should_reset_codex_model(Some(model)))
        && codex_state
            .map(|state| !crate::config::should_reset_codex_base_url(state.base_url.as_deref()))
            .unwrap_or(false);
    if has_ready_codex_state {
        return Ok(true);
    }

    if !oauth_manager.has_saved_auth()? {
        app.codex_auth_mode = None;
        return Ok(false);
    }

    let credential = oauth_manager.load_saved_credential()?;
    let credential = credential.expose_secret().trim().to_string();
    let expected_base_url = match saved_auth_mode {
        Some(crate::oauth::SavedCodexAuthMode::Chatgpt) => {
            crate::config::DEFAULT_CODEX_CHATGPT_BASE_URL
        }
        _ => crate::config::DEFAULT_CODEX_BASE_URL,
    };
    let mut changed = false;

    if app.config.provider == "codex" {
        let current_key = app
            .config
            .api_key
            .as_ref()
            .map(|value| value.expose_secret().trim());
        if current_key != Some(credential.as_str()) {
            app.config.set_api_key(credential.clone());
            changed = true;
        }
        if crate::config::should_reset_codex_model(app.config.model.as_deref()) {
            app.config
                .set_model(Some(crate::config::DEFAULT_CODEX_MODEL.to_string()));
            changed = true;
        }
        if crate::config::should_apply_codex_base_url(
            app.config.base_url.as_deref(),
            expected_base_url,
        ) {
            app.config.set_base_url(Some(expected_base_url.to_string()));
            changed = true;
        }
    } else {
        let mut codex_state = app
            .config
            .provider_states
            .get("codex")
            .cloned()
            .unwrap_or_default();
        let current_key = codex_state
            .api_key
            .as_ref()
            .map(|value| value.expose_secret().trim());
        if current_key != Some(credential.as_str()) {
            codex_state.api_key = Some(SecretString::from(credential));
            changed = true;
        }
        if crate::config::should_reset_codex_model(codex_state.model.as_deref()) {
            codex_state.model = Some(crate::config::DEFAULT_CODEX_MODEL.to_string());
            changed = true;
        }
        if crate::config::should_apply_codex_base_url(
            codex_state.base_url.as_deref(),
            expected_base_url,
        ) {
            codex_state.base_url = Some(expected_base_url.to_string());
            changed = true;
        }
        if changed {
            app.config
                .provider_states
                .insert("codex".to_string(), codex_state);
        }
    }

    if changed {
        app.config_manager.save(&app.config)?;
    }

    Ok(true)
}

pub(super) fn codex_auth_is_available(app: &TuiApp, oauth_manager: &OAuthManager) -> bool {
    if app.config.provider == "codex" && app.config.has_api_key() {
        return true;
    }
    if app
        .config
        .provider_states
        .get("codex")
        .and_then(|state| state.api_key.as_ref())
        .is_some_and(|api_key| !api_key.expose_secret().trim().is_empty())
    {
        return true;
    }
    oauth_manager.has_saved_auth().is_ok_and(|saved| saved)
}

pub(super) async fn refresh_codex_model_picker(
    app: &mut TuiApp,
    oauth_manager: &OAuthManager,
    refresh_strategy: RefreshStrategy,
) -> anyhow::Result<()> {
    match load_codex_model_catalog(oauth_manager.codex_home(), refresh_strategy).await {
        Ok(options) => {
            if options.is_empty() && app.codex_model_options.is_empty() {
                app.push_notice(
                    "Codex model catalog is empty. Check the saved login or try again.",
                );
            }
            app.set_codex_model_options(options);
        }
        Err(err) => {
            app.push_notice(format!("Failed to load Codex model catalog: {err}"));
        }
    }
    Ok(())
}

pub(super) async fn open_provider_family_overlay(
    app: &mut TuiApp,
    oauth_manager: &OAuthManager,
) -> anyhow::Result<()> {
    let entering_codex_family = matches!(app.selected_provider_family(), ProviderFamily::Codex);
    if entering_codex_family {
        oauth_manager.invalidate_saved_auth_cache();
    }
    if matches!(app.selected_provider_family(), ProviderFamily::DeepSeek) {
        app.config.select_openai_profile(
            OpenAiEndpointKind::Deepseek.default_profile_id(),
            OpenAiEndpointKind::Deepseek.label(),
            OpenAiEndpointKind::Deepseek,
        );
        if !app.config.has_api_key() {
            app.open_overlay(Overlay::ApiKeyEditor);
        } else {
            app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
        }
        return Ok(());
    }
    if matches!(app.selected_provider_family(), ProviderFamily::Gemini) {
        let home = ensure_rara_home_dir()?;
        let google_oauth = GoogleOAuthManager::new(home)?;
        if google_oauth.has_saved_auth() {
            app.open_overlay(Overlay::ModelPicker);
        } else {
            app.config.set_provider("gemini-code-assist");
            app.open_overlay(Overlay::AuthModePicker);
        }
        return Ok(());
    }
    let has_synced_codex_auth = if entering_codex_family {
        sync_codex_credential_from_auth_store(app, oauth_manager)?
    } else {
        false
    };

    if entering_codex_family
        && !has_synced_codex_auth
        && !codex_auth_is_available(app, oauth_manager)
    {
        app.config.set_provider("codex");
        app.open_overlay(Overlay::ListPicker(ListPickerKind::AuthMode));
    } else {
        if entering_codex_family {
            refresh_codex_model_picker(app, oauth_manager, RefreshStrategy::OnlineIfUncached)
                .await?;
        }
        app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
        if matches!(
            app.selected_provider_family(),
            ProviderFamily::OpenAiCompatible
        ) && app.openai_profile_needs_setup()
        {
            app.begin_openai_profile_setup();
        }
    }
    Ok(())
}

pub(super) fn should_open_codex_auth_guide(app: &TuiApp, oauth_manager: &OAuthManager) -> bool {
    app.selected_provider_family() == ProviderFamily::Codex
        && !codex_auth_is_available(app, oauth_manager)
}

#[allow(dead_code)]
fn _keep_agent_type(_agent: &mut Option<Agent>) {}
