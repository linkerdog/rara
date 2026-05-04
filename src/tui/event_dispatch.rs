use std::sync::Arc;

use super::app_event::AppEvent;
use super::auth_mode_picker::AUTH_MODE_OPTION_COUNT;
use super::command::{palette_command_by_index, palette_commands};
use super::provider_flow::{
    open_provider_family_overlay, should_open_codex_auth_guide,
    sync_codex_credential_from_auth_store,
};
use super::runtime::{
    request_running_task_cancellation, start_deepseek_model_list_task, start_oauth_task,
    start_pending_approval_task, start_rebuild_task,
};
use super::session_restore::restore_thread_by_id;
use super::state::{
    ActivePendingInteractionKind, OpenAiModelPickerAction, Overlay, PROVIDER_FAMILIES,
    ProviderFamily, TuiApp,
};
use super::submit::{apply_openai_model_picker_action, handle_submit};
use super::terminal_ui::is_ssh_session;
use crate::agent::{Agent, BashApprovalDecision};
use crate::config::DEFAULT_CODEX_BASE_URL;
use crate::oauth::{OAuthManager, SavedCodexAuthMode};

pub(crate) async fn dispatch_event(
    event: AppEvent,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
) -> anyhow::Result<bool> {
    match event {
        AppEvent::Noop => {}
        AppEvent::OpenOverlay(overlay) => app.open_overlay(overlay),
        AppEvent::CloseOverlay => app.close_overlay(),
        AppEvent::CancelRunningTask => request_running_task_cancellation(app),
        AppEvent::SubmitComposer => {
            if handle_submit(app, agent_slot, oauth_manager).await? {
                return Ok(true);
            }
        }
        AppEvent::InsertNewline => {
            app.insert_newline_in_composer();
        }
        AppEvent::InputChar(c) => {
            app.insert_active_input_char(c);
        }
        AppEvent::Backspace => {
            app.backspace_active_input();
        }
        AppEvent::DeleteForward => {
            app.delete_forward_active_input();
        }
        AppEvent::MoveCursorLeft => {
            app.move_active_input_cursor_left();
        }
        AppEvent::MoveCursorRight => {
            app.move_active_input_cursor_right();
        }
        AppEvent::MoveCursorHome => {
            app.move_active_input_cursor_home();
        }
        AppEvent::MoveCursorEnd => {
            app.move_active_input_cursor_end();
        }
        AppEvent::MoveCursorUp => {
            app.move_composer_cursor_up();
        }
        AppEvent::MoveCursorDown => {
            app.move_composer_cursor_down();
        }
        AppEvent::NavigateInputHistory(delta) => {
            app.navigate_input_history(delta);
        }
        AppEvent::ScrollTranscript(delta) => app.scroll_transcript(delta),
        AppEvent::ScrollContext(delta) => app.scroll_context(delta),
        AppEvent::MoveCommandSelection(delta) => {
            let len = palette_commands(app, app.command_query()).len();
            if len > 0 {
                let next = (app.command_palette_idx as i32 + delta).clamp(0, len as i32 - 1);
                app.command_palette_idx = next as usize;
            }
        }
        AppEvent::MoveProviderSelection(delta) => {
            let next = (app.provider_picker_idx as i32 + delta)
                .clamp(0, PROVIDER_FAMILIES.len() as i32 - 1);
            app.provider_picker_idx = next as usize;
        }
        AppEvent::MoveResumeSelection(delta) => {
            let len = app.recent_threads.len();
            if len > 0 {
                let next = (app.resume_picker_idx as i32 + delta).clamp(0, len as i32 - 1);
                app.resume_picker_idx = next as usize;
            }
        }
        AppEvent::MoveSkillsSelection(delta) => {
            let len = app.skill_picker_entries.len();
            if len > 0 {
                let next = (app.skill_picker_idx as i32 + delta).clamp(0, len as i32 - 1);
                app.skill_picker_idx = next as usize;
            }
        }
        AppEvent::ToggleSkillSelection => {
            if let Some(entry) = app.skill_picker_entries.get_mut(app.skill_picker_idx) {
                entry.enabled = !entry.enabled;
                entry.disable_model_invocation = !entry.enabled;
            }
        }
        AppEvent::MoveModelSelection(delta) => {
            let len = if matches!(app.overlay, Some(Overlay::OpenAiEndpointKindPicker)) {
                app.openai_endpoint_kind_count()
            } else {
                app.current_model_picker_len()
            };
            if len > 0 {
                let next = delta.clamp(-(len as i32), len as i32);
                if matches!(app.overlay, Some(Overlay::OpenAiEndpointKindPicker)) {
                    let idx = (app.openai_endpoint_kind_picker_idx as i32 + next)
                        .clamp(0, len as i32 - 1);
                    app.openai_endpoint_kind_picker_idx = idx as usize;
                } else {
                    let idx = (app.model_picker_idx as i32 + next).clamp(0, len as i32 - 1);
                    app.model_picker_idx = idx as usize;
                }
            }
        }
        AppEvent::MoveOpenAiProfileSelection(delta) => {
            let len = app.selected_openai_profiles().len() + 1;
            if len > 0 {
                let next = (app.openai_profile_picker_idx as i32 + delta).clamp(0, len as i32 - 1);
                app.openai_profile_picker_idx = next as usize;
            }
        }
        AppEvent::MoveReasoningEffortSelection(delta) => {
            let len = app.selected_codex_reasoning_options().len();
            if len > 0 {
                let next =
                    (app.reasoning_effort_picker_idx as i32 + delta).clamp(0, len as i32 - 1);
                app.reasoning_effort_picker_idx = next as usize;
            }
        }
        AppEvent::MoveAuthModeSelection(delta) => {
            let max_idx = AUTH_MODE_OPTION_COUNT.saturating_sub(1);
            let next = (app.auth_mode_idx as i32 + delta).clamp(0, max_idx as i32);
            app.auth_mode_idx = next as usize;
        }
        AppEvent::SetProviderSelection(idx) => {
            app.provider_picker_idx = idx.min(PROVIDER_FAMILIES.len() - 1);
            app.model_picker_idx = 0;
        }
        AppEvent::SetAuthModeSelection(idx) => {
            app.auth_mode_idx = idx.min(AUTH_MODE_OPTION_COUNT.saturating_sub(1));
        }
        AppEvent::SetReasoningEffortSelection(idx) => {
            let len = app.selected_codex_reasoning_options().len();
            app.reasoning_effort_picker_idx = idx.min(len.saturating_sub(1));
        }
        AppEvent::SelectPendingOption(idx) => {
            if let Some(interaction) = app.active_pending_interaction() {
                match interaction.kind {
                    ActivePendingInteractionKind::PlanApproval => {
                        if let 0 | 1 = idx {
                            if let Some(agent) = agent_slot.take() {
                                let continue_planning = idx == 1;
                                super::runtime::start_plan_approval_resume_task(
                                    app,
                                    continue_planning,
                                    agent,
                                );
                            }
                        }
                    }
                    ActivePendingInteractionKind::ShellApproval => {
                        if let Some(agent) = agent_slot.take() {
                            let selection = match idx {
                                0 => BashApprovalDecision::Once,
                                1 => BashApprovalDecision::Prefix,
                                2 => BashApprovalDecision::Always,
                                _ => BashApprovalDecision::Suggestion,
                            };
                            start_pending_approval_task(app, selection, agent);
                        }
                    }
                    ActivePendingInteractionKind::PlanningQuestion
                    | ActivePendingInteractionKind::ExplorationQuestion
                    | ActivePendingInteractionKind::SubAgentQuestion
                    | ActivePendingInteractionKind::RequestInput => {
                        if let Some(label) = app.pending_question_option_label(idx) {
                            if let Some(agent) = agent_slot.as_mut() {
                                agent.consume_pending_user_input(&label);
                                app.sync_snapshot(agent);
                            }
                            app.set_input(label);
                            if handle_submit(app, agent_slot, oauth_manager).await? {
                                return Ok(true);
                            }
                        }
                    }
                }
            }
        }
        AppEvent::CycleModelSelection => {
            app.cycle_local_model();
        }
        AppEvent::SaveBaseUrlInput => {
            if app.is_busy() {
                app.push_notice("Wait for the current task before saving the base URL.");
            } else {
                let value = app.base_url_input.trim();
                app.config
                    .set_base_url((!value.is_empty()).then(|| value.to_string()));
                app.config_manager.save(&app.config)?;
                app.notice = Some(format!(
                    "Saved base URL: {}",
                    app.config.base_url.as_deref().unwrap_or("unset")
                ));
                if app.openai_setup_steps.is_empty() {
                    app.close_overlay();
                } else {
                    app.advance_openai_profile_setup();
                }
            }
        }
        AppEvent::SaveApiKeyInput => {
            let value = app.api_key_input.trim();
            if app.is_busy() {
                app.push_notice("Wait for the current task before saving the API key.");
            } else if value.is_empty() && app.config.provider == "codex" {
                app.push_notice("Enter a Codex API key or press Esc to go back.");
            } else if value.is_empty() && app.selected_provider_family() == ProviderFamily::DeepSeek
            {
                app.push_notice("Enter a DeepSeek API key or press Esc to go back.");
            } else if value.is_empty() && app.openai_setup_keep_empty_api_key {
                app.notice = Some("Kept existing API key for the current profile.".into());
                app.advance_openai_profile_setup();
            } else if value.is_empty() {
                app.config.clear_api_key();
                if app.config.provider == "codex" {
                    app.codex_auth_mode = None;
                }
                app.config_manager.save(&app.config)?;
                app.notice = Some("Cleared API key for the current provider.".into());
                if app.openai_setup_steps.is_empty() {
                    app.close_overlay();
                } else {
                    app.advance_openai_profile_setup();
                }
            } else {
                let was_deepseek = app.selected_provider_family() == ProviderFamily::DeepSeek;
                app.config.set_api_key(value.to_string());
                if app.config.provider == "codex" {
                    app.codex_auth_mode = Some(SavedCodexAuthMode::ApiKey);
                    app.config
                        .apply_codex_defaults_for_base_url(DEFAULT_CODEX_BASE_URL);
                }
                app.config_manager.save(&app.config)?;
                if app.config.provider == "codex" {
                    app.notice = Some("Saved Codex API key. Rebuilding backend.".into());
                    app.overlay = None;
                    start_rebuild_task(app);
                } else if was_deepseek {
                    app.notice = Some("Saved DeepSeek API key. Loading models.".into());
                    app.overlay = None;
                    start_deepseek_model_list_task(app);
                } else {
                    app.notice = Some("Saved API key for the current provider.".into());
                    if app.openai_setup_steps.is_empty() {
                        app.close_overlay();
                    } else {
                        app.advance_openai_profile_setup();
                    }
                }
            }
        }
        AppEvent::SaveModelNameInput => {
            if app.is_busy() {
                app.push_notice("Wait for the current task before saving the model name.");
            } else {
                let value = app.model_name_input.trim();
                if value.is_empty() {
                    app.push_notice("Enter a model name or press Esc to go back.");
                } else {
                    app.config.set_model(Some(value.to_string()));
                    app.config_manager.save(&app.config)?;
                    app.notice = Some(format!("Saved model name: {}", value));
                    if app.openai_setup_steps.is_empty() {
                        app.close_overlay();
                    } else {
                        app.advance_openai_profile_setup();
                    }
                }
            }
        }
        AppEvent::SaveOpenAiProfileLabelInput => {
            if app.is_busy() {
                app.push_notice("Wait for the current task before creating a profile.");
            } else if app.selected_provider_family() != ProviderFamily::OpenAiCompatible {
                app.push_notice(
                    "OpenAI-compatible profiles are only available in that provider family.",
                );
            } else {
                let label = app.openai_profile_label_input.trim();
                if label.is_empty() {
                    app.push_notice("Enter a profile label or press Esc to go back.");
                } else if let Some(kind) = app
                    .openai_profile_label_kind
                    .or_else(|| app.selected_openai_profile_kind())
                {
                    let profile_id = app.next_openai_profile_id(kind, label);
                    app.config.select_openai_profile(profile_id, label, kind);
                    app.config_manager.save(&app.config)?;
                    app.notice = Some(format!("Created endpoint profile: {label}"));
                    app.openai_profile_label_kind = None;
                    app.begin_created_openai_profile_setup();
                }
            }
        }
        AppEvent::CreateOpenAiProfile => {
            if app.is_busy() {
                app.push_notice("Wait for the current task before creating a profile.");
            } else if app.selected_provider_family() == ProviderFamily::OpenAiCompatible {
                app.begin_openai_profile_setup();
            }
        }
        AppEvent::DeleteOpenAiProfile => {
            if app.is_busy() {
                app.push_notice("Wait for the current task before deleting a profile.");
            } else {
                apply_openai_model_picker_action(app, OpenAiModelPickerAction::DeleteProfile)?;
            }
        }
        AppEvent::RefreshDeepSeekModels => {
            if app.is_busy() {
                app.push_notice("Wait for the current task before refreshing DeepSeek models.");
            } else if app.selected_provider_family() != ProviderFamily::DeepSeek {
                app.push_notice("DeepSeek model refresh is only available in DeepSeek.");
            } else if !app.config.has_api_key() {
                app.open_overlay(Overlay::ApiKeyEditor);
            } else {
                start_deepseek_model_list_task(app);
            }
        }
        AppEvent::SelectHelpTab(tab) => {
            app.open_overlay(Overlay::Help(tab));
        }
        AppEvent::SelectStatusTab(tab) => {
            app.open_overlay(Overlay::Status(tab));
        }
        AppEvent::SetModelSelection(idx) => {
            app.model_picker_idx = idx.min(app.current_model_picker_len().saturating_sub(1));
        }
        AppEvent::SetOpenAiProfileSelection(idx) => {
            let len = app.selected_openai_profiles().len() + 1;
            app.openai_profile_picker_idx = idx.min(len.saturating_sub(1));
        }
        AppEvent::SetResumeSelection(idx) => {
            let len = app.recent_threads.len();
            if len > 0 {
                app.resume_picker_idx = idx.min(len - 1);
            }
        }
        AppEvent::EditOpenAiProfile => {
            if app.is_busy() {
                app.push_notice("Wait for the current task before editing a profile.");
            } else if app.selected_provider_family() == ProviderFamily::OpenAiCompatible {
                if app.select_openai_model_picker_profile().is_some() {
                    app.config_manager.save(&app.config)?;
                    app.begin_edit_openai_profile_setup();
                }
            }
        }
        AppEvent::ApplyOverlaySelection => match app.overlay {
            Some(Overlay::CommandPalette) => {
                let query = app.command_query();
                if let Some(spec) = palette_command_by_index(app, query, app.command_palette_idx) {
                    app.set_input(spec.usage.to_string());
                    app.close_overlay();
                    if handle_submit(app, agent_slot, oauth_manager).await? {
                        return Ok(true);
                    }
                }
            }
            Some(Overlay::ProviderPicker) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else {
                    open_provider_family_overlay(app, oauth_manager.as_ref()).await?;
                    if app.selected_provider_family() == ProviderFamily::DeepSeek
                        && app.config.has_api_key()
                        && matches!(app.overlay, Some(Overlay::ModelPicker))
                    {
                        start_deepseek_model_list_task(app);
                    }
                }
            }
            Some(Overlay::OpenAiEndpointKindPicker) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else {
                    let kind = app.selected_openai_setup_kind();
                    app.set_openai_setup_kind(kind);
                    app.config_manager.save(&app.config)?;
                }
            }
            Some(Overlay::ResumePicker) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else if let Some(thread_id) = app
                    .recent_threads
                    .get(app.resume_picker_idx)
                    .map(|session| session.metadata.session_id.clone())
                {
                    restore_thread_by_id(thread_id.as_str(), app, agent_slot)?;
                    app.close_overlay();
                }
            }
            Some(Overlay::OpenAiProfilePicker) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else if app.openai_profile_picker_idx == 0 {
                    app.openai_profile_label_kind = app.selected_openai_profile_kind();
                    app.open_overlay(Overlay::OpenAiProfileLabelEditor);
                } else if let Some((profile_id, label)) = app
                    .selected_openai_profiles()
                    .get(app.openai_profile_picker_idx - 1)
                    .cloned()
                {
                    if let Some(kind) = app.selected_openai_profile_kind() {
                        app.config
                            .select_openai_profile(profile_id, label.clone(), kind);
                        app.config_manager.save(&app.config)?;
                        app.notice = Some(format!("Selected endpoint profile: {label}"));
                        app.overlay = Some(Overlay::ModelPicker);
                    }
                }
            }
            Some(Overlay::BaseUrlEditor) => {
                if app.is_busy() {
                    app.push_notice("Wait for the current task before saving the base URL.");
                } else {
                    let value = app.base_url_input.trim();
                    app.config
                        .set_base_url((!value.is_empty()).then(|| value.to_string()));
                    app.config_manager.save(&app.config)?;
                    app.notice = Some(format!(
                        "Saved base URL: {}",
                        app.config.base_url.as_deref().unwrap_or("unset")
                    ));
                    app.close_overlay();
                }
            }
            Some(Overlay::ModelPicker) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else {
                    if app.selected_provider_family() == ProviderFamily::Codex {
                        let _ = sync_codex_credential_from_auth_store(app, oauth_manager.as_ref())?;
                    }
                    if should_open_codex_auth_guide(app, oauth_manager.as_ref()) {
                        app.select_local_model(app.model_picker_idx);
                        app.open_overlay(Overlay::AuthModePicker);
                    } else if app.selected_provider_family() == ProviderFamily::Codex {
                        app.select_local_model(app.model_picker_idx);
                        if app.selected_codex_reasoning_options().len() <= 1 {
                            app.apply_selected_codex_reasoning_effort();
                            start_rebuild_task(app);
                        } else {
                            app.open_overlay(Overlay::ReasoningEffortPicker);
                        }
                    } else if app.selected_provider_family() == ProviderFamily::OpenAiCompatible {
                        if let Some(action) = app.selected_openai_model_picker_action() {
                            apply_openai_model_picker_action(app, action)?;
                        }
                    } else if app.selected_provider_family() == ProviderFamily::DeepSeek {
                        if app.selected_deepseek_api_key_action() {
                            app.open_overlay(Overlay::ApiKeyEditor);
                        } else if app.config.has_api_key() {
                            app.select_local_model(app.model_picker_idx);
                            start_rebuild_task(app);
                        } else {
                            app.open_overlay(Overlay::ApiKeyEditor);
                        }
                    } else {
                        app.select_local_model(app.model_picker_idx);
                        start_rebuild_task(app);
                    }
                }
            }
            Some(Overlay::ReasoningEffortPicker) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else {
                    app.select_local_model(app.model_picker_idx);
                    app.apply_selected_codex_reasoning_effort();
                    start_rebuild_task(app);
                }
            }
            Some(Overlay::AuthModePicker) => match app.auth_mode_idx {
                0 => {
                    if app.is_busy() {
                        app.push_notice("A task is already running. Wait for it to finish.");
                    } else if is_ssh_session() {
                        app.push_notice("Browser login is unavailable in SSH/headless sessions. Choose device code or API key instead.");
                    } else {
                        app.close_overlay();
                        start_oauth_task(
                            app,
                            Arc::clone(oauth_manager),
                            super::state::OAuthLoginMode::Browser,
                        );
                    }
                }
                1 => {
                    if app.is_busy() {
                        app.push_notice("A task is already running. Wait for it to finish.");
                    } else {
                        app.close_overlay();
                        start_oauth_task(
                            app,
                            Arc::clone(oauth_manager),
                            super::state::OAuthLoginMode::DeviceCode,
                        );
                    }
                }
                2 => app.open_overlay(Overlay::ApiKeyEditor),
                3 => {
                    if app.is_busy() {
                        app.push_notice("A task is already running. Wait for it to finish.");
                    } else {
                        let removed = oauth_manager.clear_saved_auth()?;
                        app.config.clear_provider_api_key("codex");
                        app.codex_auth_mode = None;
                        app.config_manager.save(&app.config)?;
                        app.notice = Some(if removed {
                            "Cleared the saved provider credential.".into()
                        } else {
                            "No saved provider credential was present.".into()
                        });
                        if app.config.provider == "codex" {
                            start_rebuild_task(app);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        },
    }
    Ok(false)
}
