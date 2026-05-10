use std::sync::Arc;

use super::app_event::AppEvent;
use super::command::{palette_command_by_index, palette_commands};
use super::input_control;
#[allow(unused_imports)]
use super::list_picker;
use super::provider_flow::{
    open_provider_family_overlay, should_open_codex_auth_guide,
    sync_codex_credential_from_auth_store,
};
use super::runtime::apply_permission_mode;
use super::runtime::{start_deepseek_model_list_task, start_oauth_task, start_rebuild_task};
use super::session_restore::restore_thread_by_id;
use super::state::{
    ActivePendingInteractionKind, ListPickerKind, OpenAiModelPickerAction, Overlay, PermissionMode,
    PickerIntent, ProviderFamily, TuiApp,
};
use super::submit::{apply_openai_model_picker_action, handle_submit};
use super::terminal_ui::is_ssh_session;
use crate::agent::Agent;
use crate::config::DEFAULT_CODEX_BASE_URL;
use crate::oauth::{OAuthManager, SavedCodexAuthMode};
use crate::runtime_control::{SessionControlRequest, ShellApprovalDecision};

pub(crate) async fn dispatch_event(
    event: AppEvent,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
) -> anyhow::Result<bool> {
    match event {
        AppEvent::Noop => {}
        AppEvent::OpenOverlay(overlay) => app.open_overlay(overlay),
        AppEvent::CloseOverlay => {
            app.close_overlay();
        }
        AppEvent::CancelRunningTask => {
            input_control::handle_session_control(app, SessionControlRequest::CancelCurrentTurn);
        }
        AppEvent::ClearComposer => {
            app.bottom_pane.input.clear();
            app.bottom_pane.input_cursor_offset = None;
        }
        AppEvent::ToggleSidebar => {
            app.sidebar_visible = !app.sidebar_visible;
        }
        AppEvent::SubmitComposer => {
            if resume_pending_shell_approval_after_full_access(app, agent_slot) {
                return Ok(false);
            }
            if handle_submit(app, agent_slot, oauth_manager).await? {
                return Ok(true);
            }
        }
        AppEvent::InsertNewline => {
            app.insert_newline_in_composer();
        }
        AppEvent::InputChar(c) => {
            if app.bottom_pane.input.is_empty() {
                app.transcript_scroll = 0;
            }
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
            if resume_pending_shell_approval_after_full_access(app, agent_slot) {
                return Ok(false);
            }
            if let Some(interaction) = app.active_pending_interaction() {
                match interaction.kind {
                    ActivePendingInteractionKind::PlanApproval => {
                        if let 0 | 1 = idx {
                            input_control::answer_plan_approval(app, agent_slot, idx == 0);
                        }
                    }
                    ActivePendingInteractionKind::ShellApproval => {
                        let selection = match idx {
                            0 => ShellApprovalDecision::Once,
                            1 => ShellApprovalDecision::Prefix,
                            2 => ShellApprovalDecision::Always,
                            _ => ShellApprovalDecision::Suggestion,
                        };
                        input_control::answer_shell_approval(app, agent_slot, selection);
                    }
                    ActivePendingInteractionKind::PlanningQuestion
                    | ActivePendingInteractionKind::ExplorationQuestion
                    | ActivePendingInteractionKind::SubAgentQuestion
                    | ActivePendingInteractionKind::RequestInput => {
                        if let Some(label) = app.pending_question_option_label(idx) {
                            if let Some(agent) = agent_slot.take() {
                                input_control::answer_pending_input(app, agent_slot, agent, label);
                            } else {
                                app.push_notice(
                                    "Request input is still preparing. Try the shortcut again.",
                                );
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
                app.bottom_pane.notice = Some(format!(
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
                app.bottom_pane.notice =
                    Some("Kept existing API key for the current profile.".into());
                app.advance_openai_profile_setup();
            } else if value.is_empty() {
                app.config.clear_api_key();
                if app.config.provider == "codex" {
                    app.codex_auth_mode = None;
                }
                app.config_manager.save(&app.config)?;
                app.bottom_pane.notice = Some("Cleared API key for the current provider.".into());
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
                    app.bottom_pane.notice =
                        Some("Saved Codex API key. Rebuilding backend.".into());
                    app.close_overlay();
                    start_rebuild_task(app);
                } else if was_deepseek {
                    app.bottom_pane.notice = Some("Saved DeepSeek API key. Loading models.".into());
                    app.close_overlay();
                    start_deepseek_model_list_task(app);
                } else {
                    app.bottom_pane.notice = Some("Saved API key for the current provider.".into());
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
                app.config
                    .set_model((!value.is_empty()).then(|| value.to_string()));
                app.config_manager.save(&app.config)?;
                app.bottom_pane.notice = Some(format!(
                    "Saved model name: {}",
                    app.config.model.as_deref().unwrap_or("unset")
                ));
                if app.openai_setup_steps.is_empty() {
                    app.close_overlay();
                } else {
                    app.advance_openai_profile_setup();
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
                    app.bottom_pane.notice = Some(format!("Created endpoint profile: {label}"));
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
            } else if app.selected_provider_family() == ProviderFamily::OpenAiCompatible
                && app.select_openai_model_picker_profile().is_some()
            {
                app.config_manager.save(&app.config)?;
                app.begin_edit_openai_profile_setup();
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
                    // Save the command text before close_overlay, which clears
                    // the composer input for CommandPalette to prevent immediate
                    // re-open via sync_command_palette_with_input.
                    let usage = spec.usage.to_string();
                    app.close_overlay();
                    app.bottom_pane.input = usage;
                    app.bottom_pane.input_cursor_offset = None;
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
                    app.bottom_pane.notice = Some(format!(
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
                            // If opened from /model and provider was just configured, jump to model.
                            if app.picker_intent == Some(PickerIntent::SwitchModel)
                                && !app.config.provider.is_empty()
                            {
                                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
                            }
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
                        ListPickerKind::UnifiedModel => {
                            let idx = app.model_picker_idx;
                            let presets = app.all_unified_model_presets();
                            let Some(preset) = presets.get(idx).cloned() else {
                                return Ok(false);
                            };

                            app.select_unified_model(idx);

                            match preset.family {
                                ProviderFamily::Codex => {
                                    let _ = sync_codex_credential_from_auth_store(
                                        app,
                                        oauth_manager.as_ref(),
                                    )?;
                                    if should_open_codex_auth_guide(app, oauth_manager.as_ref()) {
                                        app.open_overlay(Overlay::ListPicker(
                                            ListPickerKind::AuthMode,
                                        ));
                                    } else {
                                        if app.selected_codex_reasoning_options().len() <= 1 {
                                            app.apply_selected_codex_reasoning_effort();
                                            start_rebuild_task(app);
                                        } else {
                                            app.open_overlay(Overlay::ListPicker(
                                                ListPickerKind::ReasoningEffort,
                                            ));
                                        }
                                    }
                                }
                                ProviderFamily::OpenAiCompatible => {
                                    if app.openai_profile_needs_setup() {
                                        app.begin_active_openai_profile_setup();
                                    } else {
                                        start_rebuild_task(app);
                                    }
                                }
                                ProviderFamily::DeepSeek | ProviderFamily::Gemini => {
                                    if !app.config.has_api_key() {
                                        app.open_overlay(Overlay::ApiKeyEditor);
                                    } else {
                                        start_rebuild_task(app);
                                    }
                                }
                                ProviderFamily::CandleLocal => {
                                    app.push_notice("Local models (alpha) are for preview only.");
                                    app.close_overlay();
                                }
                                _ => {
                                    start_rebuild_task(app);
                                }
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
                                app.bottom_pane.notice = Some(
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
                                && let Some(kind) = app.selected_openai_profile_kind()
                            {
                                app.config
                                    .select_openai_profile(profile_id, label.clone(), kind);
                                app.config_manager.save(&app.config)?;
                                app.bottom_pane.notice =
                                    Some(format!("Selected endpoint profile: {label}"));
                                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
                            }
                        }
                        ListPickerKind::ApprovalDecision => {
                            let selection = match app.approval_picker_idx {
                                0 => ShellApprovalDecision::Once,
                                1 => ShellApprovalDecision::Prefix,
                                2 => ShellApprovalDecision::Always,
                                _ => ShellApprovalDecision::Suggestion,
                            };
                            input_control::answer_shell_approval(app, agent_slot, selection);
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
                    app.close_overlay();
                    if !resume_pending_shell_approval_after_full_access(app, agent_slot) {
                        app.push_notice(format!("Permission mode: {label}."));
                    }
                }
            }
            _ => {}
        },
    }
    Ok(false)
}

fn resume_pending_shell_approval_after_full_access(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
) -> bool {
    if app.permission_mode != PermissionMode::FullAccess
        || !app.active_pending_interaction().is_some_and(|interaction| {
            interaction.kind == ActivePendingInteractionKind::ShellApproval
        })
        || app.is_busy()
    {
        return false;
    }

    if agent_slot.is_some() {
        input_control::answer_shell_approval(app, agent_slot, ShellApprovalDecision::Once);
    } else {
        app.push_notice("Permission mode: full-access. Approval is still preparing.");
    }
    true
}
