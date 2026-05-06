use std::sync::Arc;

use super::app_event::AppEvent;
use super::auth_mode_picker::AUTH_MODE_OPTION_COUNT;
use super::command::{palette_command_by_index, palette_commands};
#[allow(unused_imports)]
use super::list_picker;
use super::provider_flow::{
    open_provider_family_overlay, should_open_codex_auth_guide,
    sync_codex_credential_from_auth_store,
};
use super::runtime::apply_permission_mode;
use super::runtime::{
    request_running_task_cancellation, start_deepseek_model_list_task, start_oauth_task,
    start_pending_approval_task, start_rebuild_task,
};
use super::session_restore::restore_thread_by_id;
use super::state::{
    ActivePendingInteractionKind, ListPickerKind, OpenAiModelPickerAction, Overlay,
    PROVIDER_FAMILIES, PermissionMode, ProviderFamily, TuiApp,
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
        AppEvent::ClearComposer => {
            app.input.clear();
            app.input_cursor_offset = None;
        }
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
        AppEvent::MoveListPickerSelection(delta) => {
            let Some(Overlay::ListPicker(kind)) = app.overlay else {
                return Ok(false);
            };
            let max = kind.item_count(app).saturating_sub(1) as i32;
            let next = (kind.idx(app) as i32 + delta).clamp(0, max);
            kind.set_idx(app, next as usize);
        }
        AppEvent::SetListPickerSelection(idx) => {
            let Some(Overlay::ListPicker(kind)) = app.overlay else {
                return Ok(false);
            };
            kind.set_idx(app, idx);
        }
        AppEvent::MovePermissionSelection(delta) => {
            let max_idx = 3i32;
            let next = (app.permission_picker_idx as i32 + delta).clamp(0, max_idx);
            app.permission_picker_idx = next as usize;
        }
        AppEvent::SetPermissionSelection(idx) => {
            app.permission_picker_idx = idx.min(3usize);
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
                    _ => {}
                }
            }
        }
        AppEvent::CycleModelSelection => {
            app.cycle_local_model();
        }
        AppEvent::SaveBaseUrlInput => {
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
        AppEvent::SaveApiKeyInput => {
            let value = app.api_key_input.trim();
            if value.is_empty() {
                app.notice = Some("API key unchanged.".into());
            } else {
                app.config.set_api_key(value);
                app.config_manager.save(&app.config)?;
                app.notice = Some("Saved API key.".into());
            }
            app.close_overlay();
            if app.config.provider == "codex" {
                start_rebuild_task(app);
            }
        }
        AppEvent::SaveModelNameInput => {
            let value = app.model_name_input.trim();
            app.config
                .set_model((!value.is_empty()).then(|| value.to_string()));
            app.config_manager.save(&app.config)?;
            app.notice = Some(format!(
                "Saved model name: {}",
                app.config.model.as_deref().unwrap_or("unset")
            ));
            app.close_overlay();
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
        AppEvent::DeleteOpenAiProfile => {
            if app.is_busy() {
                app.push_notice("Wait for the current task before deleting a profile.");
            } else if app.selected_provider_family() == ProviderFamily::OpenAiCompatible {
                apply_openai_model_picker_action(app, OpenAiModelPickerAction::DeleteProfile)?;
            }
        }
        AppEvent::RefreshDeepSeekModels => {
            start_deepseek_model_list_task(app);
        }
        AppEvent::SelectHelpTab(tab) => {
            app.open_overlay(Overlay::Help(tab));
        }
        AppEvent::SelectStatusTab(tab) => {
            app.open_overlay(Overlay::Status(tab));
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
            Some(Overlay::ListPicker(kind)) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else {
                    match kind {
                        ListPickerKind::Provider => {
                            open_provider_family_overlay(app, oauth_manager.as_ref()).await?;
                        }
                        ListPickerKind::Model => {
                            if app.selected_provider_family() == ProviderFamily::Codex {
                                let _ = sync_codex_credential_from_auth_store(
                                    app,
                                    oauth_manager.as_ref(),
                                )?;
                            }
                            if should_open_codex_auth_guide(app, oauth_manager.as_ref()) {
                                app.select_local_model(app.model_picker_idx);
                                app.open_overlay(Overlay::ListPicker(ListPickerKind::AuthMode));
                            } else if app.selected_provider_family() == ProviderFamily::Codex {
                                app.select_local_model(app.model_picker_idx);
                                if app.selected_codex_reasoning_options().len() <= 1 {
                                    app.apply_selected_codex_reasoning_effort();
                                    start_rebuild_task(app);
                                } else {
                                    app.open_overlay(Overlay::ListPicker(
                                        ListPickerKind::ReasoningEffort,
                                    ));
                                }
                            } else if app.selected_provider_family()
                                == ProviderFamily::OpenAiCompatible
                            {
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
                        ListPickerKind::AuthMode => match app.auth_mode_idx {
                            0 if !is_ssh_session() => {
                                app.close_overlay();
                                start_oauth_task(
                                    app,
                                    Arc::clone(oauth_manager),
                                    super::state::OAuthLoginMode::Browser,
                                );
                            }
                            0 => app.push_notice("Browser login unavailable in SSH/headless."),
                            1 => {
                                app.close_overlay();
                                start_oauth_task(
                                    app,
                                    Arc::clone(oauth_manager),
                                    super::state::OAuthLoginMode::DeviceCode,
                                );
                            }
                            2 => app.open_overlay(Overlay::ApiKeyEditor),
                            3 => {
                                let removed = oauth_manager.clear_saved_auth()?;
                                app.config.clear_provider_api_key("codex");
                                app.codex_auth_mode = None;
                                app.config_manager.save(&app.config)?;
                                app.notice = Some(
                                    if removed {
                                        "Cleared saved credential."
                                    } else {
                                        "No saved credential present."
                                    }
                                    .into(),
                                );
                                if app.config.provider == "codex" {
                                    start_rebuild_task(app);
                                }
                            }
                            _ => {}
                        },
                        ListPickerKind::ReasoningEffort => {
                            app.select_local_model(app.model_picker_idx);
                            app.apply_selected_codex_reasoning_effort();
                            start_rebuild_task(app);
                        }
                        ListPickerKind::Resume => {
                            if let Some(thread_id) = app
                                .recent_threads
                                .get(app.resume_picker_idx)
                                .map(|session| session.metadata.session_id.clone())
                            {
                                restore_thread_by_id(thread_id.as_str(), app, agent_slot)?;
                                app.close_overlay();
                            }
                        }
                        ListPickerKind::OpenAiEndpointKind => {
                            let k = app.selected_openai_setup_kind();
                            app.set_openai_setup_kind(k);
                            app.config_manager.save(&app.config)?;
                        }
                        ListPickerKind::OpenAiProfile => {
                            if app.openai_profile_picker_idx == 0 {
                                app.openai_profile_label_kind = app.selected_openai_profile_kind();
                                app.open_overlay(Overlay::OpenAiProfileLabelEditor);
                            } else if let Some((profile_id, label)) = app
                                .selected_openai_profiles()
                                .get(app.openai_profile_picker_idx - 1)
                                .cloned()
                            {
                                if let Some(kind) = app.selected_openai_profile_kind() {
                                    app.config.select_openai_profile(
                                        profile_id,
                                        label.clone(),
                                        kind,
                                    );
                                    app.config_manager.save(&app.config)?;
                                    app.notice =
                                        Some(format!("Selected endpoint profile: {label}"));
                                    app.overlay = Some(Overlay::ListPicker(ListPickerKind::Model));
                                }
                            }
                        }
                    }
                }
            }
            Some(Overlay::PermissionPicker) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else {
                    let mode = match app.permission_picker_idx {
                        0 => PermissionMode::Auto,
                        1 => PermissionMode::AcceptEdits,
                        2 => PermissionMode::ReadOnly,
                        3 => PermissionMode::FullAccess,
                        _ => PermissionMode::Auto,
                    };
                    apply_permission_mode(app, agent_slot, mode);
                    app.permission_mode = mode;
                    let label = mode.label();
                    app.push_notice(format!("Permission mode: {label}."));
                    app.close_overlay();
                }
            }
            _ => {}
        },
    }
    Ok(false)
}
