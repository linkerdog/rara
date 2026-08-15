use std::sync::Arc;

use rara_provider_catalog::ModelCatalogProvider;

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
use super::runtime::start_oauth_task;
use super::runtime_port::{RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand};
use super::session_restore::restore_thread_by_id;
use super::state::{
    ActivePendingInteractionKind, ApiKeyTarget, ListPickerKind, OpenAiModelPickerAction, Overlay,
    PermissionMode, ProviderFamily, TuiApp,
};
use super::submit::{apply_openai_model_picker_action, handle_submit, handle_submit_with_port};
use super::terminal_ui::is_ssh_session;
use crate::agent::Agent;
use crate::config::DEFAULT_CODEX_BASE_URL;
use crate::oauth::{OAuthManager, SavedCodexAuthMode};
use crate::runtime_control::{SessionControlRequest, ShellApprovalDecision};

#[cfg(test)]
pub(crate) async fn dispatch_event(
    event: AppEvent,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
) -> anyhow::Result<bool> {
    dispatch_event_inner(event, app, agent_slot, oauth_manager, None).await
}

pub(crate) async fn dispatch_event_with_runtime(
    event: AppEvent,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
    runtime_port: &dyn RuntimeClientPort,
) -> anyhow::Result<bool> {
    dispatch_event_inner(event, app, agent_slot, oauth_manager, Some(runtime_port)).await
}

async fn dispatch_event_inner(
    event: AppEvent,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<bool> {
    match event {
        AppEvent::Noop => {}
        AppEvent::OpenOverlay(overlay) => app.open_overlay(overlay),
        AppEvent::CloseOverlay => {
            if matches!(app.overlay, Some(Overlay::ModelSearch)) {
                app.model_search_query.clear();
            }
            if matches!(
                app.overlay,
                Some(Overlay::ListPicker(ListPickerKind::Resume))
            ) {
                app.clear_resume_search();
            }
            app.dismiss_overlay();
        }
        AppEvent::CancelRunningTask => {
            if let Some(runtime_port) = runtime_port {
                runtime_port
                    .send(RuntimeCommand::Session(
                        SessionControlRequest::CancelCurrentTurn,
                    ))
                    .await?;
            } else {
                input_control::handle_session_control(
                    app,
                    SessionControlRequest::CancelCurrentTurn,
                );
            }
        }
        AppEvent::ClearComposer => {
            app.bottom_pane.input.clear();
            app.bottom_pane.input_cursor_offset = None;
        }
        AppEvent::ToggleSidebar => {
            app.sidebar_visible = !app.sidebar_visible;
        }
        AppEvent::ToggleThinking => {
            app.thinking_collapsed = !app.thinking_collapsed;
        }
        AppEvent::SubmitComposer => {
            app.bottom_pane.expand_large_paste();
            if resume_pending_shell_approval_after_full_access(app, agent_slot, runtime_port)
                .await?
            {
                return Ok(false);
            }
            let should_quit = if let Some(runtime_port) = runtime_port {
                handle_submit_with_port(app, agent_slot, oauth_manager, runtime_port).await?
            } else {
                handle_submit(app, agent_slot, oauth_manager).await?
            };
            if should_quit {
                return Ok(true);
            }
        }
        AppEvent::InsertNewline => {
            app.insert_newline_in_composer();
        }
        AppEvent::InputChar(c) => {
            if matches!(app.overlay, Some(Overlay::ModelSearch)) {
                app.model_search_query.push(c);
                app.model_search_idx = 0;
                return Ok(false);
            }
            if matches!(
                app.overlay,
                Some(Overlay::ListPicker(ListPickerKind::Resume))
            ) {
                app.push_resume_search_char(c);
                return Ok(false);
            }
            if app.bottom_pane.input.is_empty() {
                app.transcript_scroll = 0;
            }
            app.insert_active_input_char(c);
        }
        AppEvent::Backspace => {
            if matches!(app.overlay, Some(Overlay::ModelSearch)) {
                app.model_search_query.pop();
                return Ok(false);
            }
            if matches!(
                app.overlay,
                Some(Overlay::ListPicker(ListPickerKind::Resume))
            ) {
                app.pop_resume_search_char();
                return Ok(false);
            }
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
        AppEvent::StartTranscriptSelection(position) => {
            app.transcript_selection.start(position);
        }
        AppEvent::DragTranscriptSelection(position) => {
            app.transcript_selection.drag(position);
        }
        AppEvent::FinishTranscriptSelection(position) => {
            if let Some(text) = app.transcript_selection.finish(position) {
                match crate::tui::clipboard::copy_text(text.as_str()) {
                    Ok(()) => app.push_notice("Copied transcript selection to clipboard."),
                    Err(err) => {
                        app.push_notice(format!("Failed to copy transcript selection: {err}"))
                    }
                }
            }
        }
        AppEvent::ScrollContext(delta) => app.scroll_context(delta),
        AppEvent::MoveCommandSelection(delta) => {
            if matches!(app.overlay, Some(Overlay::ModelSearch)) {
                let presets = app.available_unified_model_presets();
                let q = app.model_search_query.to_ascii_lowercase();
                let count = if q.is_empty() {
                    presets.len()
                } else {
                    presets
                        .iter()
                        .filter(|p| {
                            p.model_label.to_ascii_lowercase().contains(&q)
                                || p.provider_label.to_ascii_lowercase().contains(&q)
                        })
                        .count()
                };
                if count > 0 {
                    let next = (app.model_search_idx as i32 + delta).clamp(0, count as i32 - 1);
                    app.model_search_idx = next as usize;
                }
                return Ok(false);
            }
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
        AppEvent::MoveApprovalSelection(delta) => {
            if app.active_pending_interaction().is_some_and(|interaction| {
                matches!(
                    interaction.kind,
                    ActivePendingInteractionKind::ShellApproval
                        | ActivePendingInteractionKind::PlanApproval
                )
            }) {
                let max_idx = app.active_pending_option_count().saturating_sub(1) as i32;
                let next = (app.approval_picker_idx as i32 + delta).clamp(0, max_idx);
                app.approval_picker_idx = next as usize;
            }
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
            if resume_pending_shell_approval_after_full_access(app, agent_slot, runtime_port)
                .await?
            {
                return Ok(false);
            }
            if let Some(interaction) = app.active_pending_interaction() {
                match interaction.kind {
                    ActivePendingInteractionKind::PlanApproval => {
                        if let Some(decision) = input_control::plan_approval_decision_for_index(idx)
                        {
                            if let Some(runtime_port) = runtime_port {
                                runtime_port
                                    .send(RuntimeCommand::Input(
                                        crate::runtime_control::InputControlRequest::AnswerPlanApproval {
                                            decision,
                                            feedback: None,
                                        },
                                    ))
                                    .await?;
                            } else {
                                input_control::answer_plan_approval(app, agent_slot, decision);
                            }
                        } else {
                            app.push_notice("Invalid plan approval option.");
                        }
                    }
                    ActivePendingInteractionKind::ShellApproval => {
                        let selection = match idx {
                            0 => ShellApprovalDecision::Once,
                            1 => ShellApprovalDecision::Prefix,
                            2 => ShellApprovalDecision::Always,
                            _ => ShellApprovalDecision::Suggestion,
                        };
                        if let Some(runtime_port) = runtime_port {
                            runtime_port
                                .send(RuntimeCommand::Input(
                                    crate::runtime_control::InputControlRequest::AnswerShellApproval {
                                        decision: selection,
                                    },
                                ))
                                .await?;
                        } else {
                            input_control::answer_shell_approval(app, agent_slot, selection);
                        }
                    }
                    ActivePendingInteractionKind::PlanningQuestion
                    | ActivePendingInteractionKind::ExplorationQuestion
                    | ActivePendingInteractionKind::SubAgentQuestion
                    | ActivePendingInteractionKind::RequestInput => {
                        if let Some(label) = app.pending_question_option_label(idx) {
                            if let Some(runtime_port) = runtime_port {
                                runtime_port
                                        .send(RuntimeCommand::Input(
                                            crate::runtime_control::InputControlRequest::AnswerPendingInput {
                                                answer: label,
                                            },
                                        ))
                                        .await?;
                            } else if let Some(agent) = agent_slot.take() {
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
                    app.dismiss_overlay();
                } else {
                    app.advance_openai_profile_setup();
                }
            }
        }
        AppEvent::SaveApiKeyInput => {
            let Some(Overlay::ApiKeyEditor(target)) = app.overlay else {
                app.push_notice("API key editor is no longer active.");
                return Ok(false);
            };
            let value = app.api_key_input.trim().to_string();
            if app.is_busy() {
                app.push_notice("Wait for the current task before saving the API key.");
            } else if value.is_empty() && target != ApiKeyTarget::OpenAiCompatible {
                app.push_notice(format!(
                    "Enter a {} API key or press Esc to go back.",
                    target.label()
                ));
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
                    app.dismiss_overlay();
                } else {
                    app.advance_openai_profile_setup();
                }
            } else {
                let codex_is_active = app.config.provider == "codex";
                match target {
                    ApiKeyTarget::Codex => app.config.set_provider_api_key("codex", value),
                    ApiKeyTarget::DeepSeek => app.config.set_provider_api_key("deepseek", value),
                    ApiKeyTarget::Kimi => app.config.set_provider_api_key("kimi", value),
                    ApiKeyTarget::KimiCoding => {
                        app.config.set_provider_api_key("kimi-coding", value)
                    }
                    ApiKeyTarget::OpenAiCompatible => app.config.set_api_key(value),
                    ApiKeyTarget::Gemini => app.config.set_provider_api_key("gemini", value),
                }
                if target == ApiKeyTarget::Codex {
                    app.codex_auth_mode = Some(SavedCodexAuthMode::ApiKey);
                    if codex_is_active {
                        app.config
                            .apply_codex_defaults_for_base_url(DEFAULT_CODEX_BASE_URL);
                    }
                }
                app.config_manager.save(&app.config)?;
                if target == ApiKeyTarget::Codex && codex_is_active {
                    app.bottom_pane.notice =
                        Some("Saved Codex API key. Rebuilding backend.".into());
                    app.dismiss_overlay();
                    request_maintenance(app, runtime_port, RuntimeMaintenanceCommand::Rebuild)
                        .await?;
                } else if target == ApiKeyTarget::DeepSeek {
                    app.bottom_pane.notice = Some("Saved DeepSeek API key. Loading models.".into());
                    app.dismiss_overlay();
                    request_maintenance(
                        app,
                        runtime_port,
                        RuntimeMaintenanceCommand::RefreshModelCatalog(
                            ModelCatalogProvider::DeepSeek,
                        ),
                    )
                    .await?;
                } else if target == ApiKeyTarget::Kimi {
                    app.bottom_pane.notice =
                        Some("Saved Moonshot AI API key. Loading models.".into());
                    app.dismiss_overlay();
                    request_maintenance(
                        app,
                        runtime_port,
                        RuntimeMaintenanceCommand::RefreshModelCatalog(ModelCatalogProvider::Kimi),
                    )
                    .await?;
                } else {
                    app.bottom_pane.notice = Some(
                        match target {
                            ApiKeyTarget::Codex => "Saved Codex API key.",
                            ApiKeyTarget::DeepSeek => "Saved DeepSeek API key.",
                            ApiKeyTarget::Kimi => "Saved Moonshot AI API key.",
                            ApiKeyTarget::KimiCoding => "Saved Kimi For Coding API key.",
                            ApiKeyTarget::OpenAiCompatible => {
                                "Saved API key for the current endpoint profile."
                            }
                            ApiKeyTarget::Gemini => "Saved Gemini API key.",
                        }
                        .into(),
                    );
                    if app.openai_setup_steps.is_empty() {
                        app.dismiss_overlay();
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
                    app.dismiss_overlay();
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
                apply_openai_model_picker_action(
                    app,
                    OpenAiModelPickerAction::DeleteProfile,
                    runtime_port,
                )
                .await?;
            }
        }
        AppEvent::SelectHelpTab(tab) => {
            app.open_overlay(Overlay::Help(tab));
        }
        AppEvent::SelectStatusTab(tab) => {
            app.open_overlay(Overlay::Status(tab));
        }
        AppEvent::CycleResumeSort => {
            app.cycle_resume_sort();
        }
        AppEvent::ClearResumeSearch => {
            if matches!(
                app.overlay,
                Some(Overlay::ListPicker(ListPickerKind::Resume))
            ) && !app.resume_search_query.is_empty()
            {
                app.clear_resume_search();
            } else {
                app.dismiss_overlay();
            }
        }

        AppEvent::ApplyOverlaySelection => match app.overlay {
            Some(Overlay::ModelSearch) => {
                let presets = app.available_unified_model_presets();
                let q = app.model_search_query.to_ascii_lowercase();
                let filtered: Vec<_> = if q.is_empty() {
                    presets.iter().collect()
                } else {
                    presets
                        .iter()
                        .filter(|p| {
                            p.model_label.to_ascii_lowercase().contains(&q)
                                || p.provider_label.to_ascii_lowercase().contains(&q)
                        })
                        .collect()
                };
                if let Some(preset) = filtered.get(app.model_search_idx) {
                    let all = app.all_unified_model_presets();
                    if let Some(global_idx) = all
                        .iter()
                        .position(|p| p.model_id == preset.model_id && p.family == preset.family)
                    {
                        app.dismiss_overlay();
                        app.model_search_query.clear();
                        app.select_unified_model(global_idx);
                    }
                }
            }
            Some(Overlay::CommandPalette) => {
                let query = app.command_query();
                if let Some(spec) = palette_command_by_index(app, query, app.command_palette_idx) {
                    // Save the command text before close_overlay, which clears
                    // the composer input for CommandPalette to prevent immediate
                    // re-open via sync_command_palette_with_input.
                    let usage = spec.usage.to_string();
                    app.dismiss_overlay();
                    app.bottom_pane.input = usage;
                    app.bottom_pane.input_cursor_offset = None;
                    let should_quit = if let Some(runtime_port) = runtime_port {
                        handle_submit_with_port(app, agent_slot, oauth_manager, runtime_port)
                            .await?
                    } else {
                        handle_submit(app, agent_slot, oauth_manager).await?
                    };
                    if should_quit {
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
                    app.dismiss_overlay();
                }
            }
            Some(Overlay::ListPicker(kind)) => {
                if app.is_busy() {
                    app.push_notice("A task is already running. Wait for it to finish.");
                } else {
                    match kind {
                        ListPickerKind::Provider => {
                            open_provider_family_overlay(app);
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
                                    request_maintenance(
                                        app,
                                        runtime_port,
                                        RuntimeMaintenanceCommand::Rebuild,
                                    )
                                    .await?;
                                } else {
                                    app.open_overlay(Overlay::ListPicker(
                                        ListPickerKind::ReasoningEffort,
                                    ));
                                }
                            } else if app.selected_provider_family()
                                == ProviderFamily::OpenAiCompatible
                            {
                                if let Some(action) = app.selected_openai_model_picker_action() {
                                    apply_openai_model_picker_action(app, action, runtime_port)
                                        .await?;
                                }
                            } else if app.selected_provider_family() == ProviderFamily::DeepSeek {
                                if app.selected_deepseek_api_key_action() {
                                    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek));
                                } else if app.config.has_api_key() {
                                    app.select_local_model(app.model_picker_idx);
                                    app.config.reasoning_effort = Some("max".to_string());
                                    request_maintenance(
                                        app,
                                        runtime_port,
                                        RuntimeMaintenanceCommand::Rebuild,
                                    )
                                    .await?;
                                } else {
                                    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek));
                                }
                            } else if app.selected_provider_family() == ProviderFamily::Kimi {
                                if app.selected_kimi_api_key_action() {
                                    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Kimi));
                                } else if app.config.has_api_key() {
                                    app.select_local_model(app.model_picker_idx);
                                    request_maintenance(
                                        app,
                                        runtime_port,
                                        RuntimeMaintenanceCommand::Rebuild,
                                    )
                                    .await?;
                                } else {
                                    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Kimi));
                                }
                            } else if app.selected_provider_family() == ProviderFamily::KimiCoding {
                                if app.config.has_api_key() {
                                    app.select_local_model(app.model_picker_idx);
                                    request_maintenance(
                                        app,
                                        runtime_port,
                                        RuntimeMaintenanceCommand::Rebuild,
                                    )
                                    .await?;
                                } else {
                                    app.open_overlay(Overlay::ApiKeyEditor(
                                        ApiKeyTarget::KimiCoding,
                                    ));
                                }
                            } else {
                                app.select_local_model(app.model_picker_idx);
                                request_maintenance(
                                    app,
                                    runtime_port,
                                    RuntimeMaintenanceCommand::Rebuild,
                                )
                                .await?;
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
                                            request_maintenance(
                                                app,
                                                runtime_port,
                                                RuntimeMaintenanceCommand::Rebuild,
                                            )
                                            .await?;
                                        } else {
                                            app.open_overlay(Overlay::ListPicker(
                                                ListPickerKind::ReasoningEffort,
                                            ));
                                        }
                                    }
                                }
                                ProviderFamily::OpenAiCompatible
                                    if app.openai_profile_needs_setup() =>
                                {
                                    app.begin_active_openai_profile_setup();
                                }
                                ProviderFamily::DeepSeek if !app.config.has_api_key() => {
                                    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek))
                                }
                                ProviderFamily::Kimi if !app.config.has_api_key() => {
                                    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Kimi))
                                }
                                ProviderFamily::KimiCoding if !app.config.has_api_key() => app
                                    .open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::KimiCoding)),
                                ProviderFamily::Gemini if !app.config.has_api_key() => {
                                    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Gemini))
                                }
                                ProviderFamily::CandleLocal => {
                                    app.push_notice("Local models (alpha) are for preview only.");
                                    app.dismiss_overlay();
                                }
                                _ => {
                                    if preset.family == ProviderFamily::DeepSeek {
                                        app.config.reasoning_effort = Some("max".to_string());
                                    }
                                    request_maintenance(
                                        app,
                                        runtime_port,
                                        RuntimeMaintenanceCommand::Rebuild,
                                    )
                                    .await?;
                                }
                            }
                        }
                        ListPickerKind::AuthMode => match app.auth_mode_idx {
                            0 if !is_ssh_session() => {
                                app.dismiss_overlay();
                                start_oauth_task(
                                    app,
                                    Arc::clone(oauth_manager),
                                    super::state::OAuthLoginMode::Browser,
                                );
                            }
                            0 => app.push_notice("Browser login unavailable in SSH/headless."),
                            1 => {
                                app.dismiss_overlay();
                                start_oauth_task(
                                    app,
                                    Arc::clone(oauth_manager),
                                    super::state::OAuthLoginMode::DeviceCode,
                                );
                            }
                            2 => app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Codex)),
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
                                    request_maintenance(
                                        app,
                                        runtime_port,
                                        RuntimeMaintenanceCommand::Rebuild,
                                    )
                                    .await?;
                                }
                            }
                            _ => {}
                        },
                        ListPickerKind::ReasoningEffort => {
                            app.select_local_model(app.model_picker_idx);
                            app.apply_selected_codex_reasoning_effort();
                            request_maintenance(
                                app,
                                runtime_port,
                                RuntimeMaintenanceCommand::Rebuild,
                            )
                            .await?;
                        }
                        ListPickerKind::NowledgeMem => {
                            let mode_label = {
                                let config = &mut app.config.builtin_plugins.nowledge_mem;
                                let was_cloud =
                                    config.mode == crate::config::NowledgeMemMode::Cloud;
                                match app.nowledge_mem_picker_idx {
                                    0 => config.enabled = false,
                                    1 => {
                                        config.enabled = true;
                                        config.mode = crate::config::NowledgeMemMode::Local;
                                    }
                                    2 => {
                                        config.enabled = true;
                                        config.mode = crate::config::NowledgeMemMode::Cloud;
                                        if !was_cloud {
                                            config.url =
                                                crate::config::DEFAULT_NOWLEDGE_MEM_CLOUD_URL
                                                    .to_string();
                                        }
                                    }
                                    _ => return Ok(false),
                                }
                                if config.enabled {
                                    config.mode_label().to_string()
                                } else {
                                    "disabled".to_string()
                                }
                            };
                            app.config_manager.save(&app.config)?;
                            app.bottom_pane.notice = Some(format!(
                                "Saved Nowledge Mem {} configuration. Rebuilding runtime.",
                                mode_label
                            ));
                            app.dismiss_overlay();
                            request_maintenance(
                                app,
                                runtime_port,
                                RuntimeMaintenanceCommand::Rebuild,
                            )
                            .await?;
                        }
                        ListPickerKind::Resume => {
                            if let Some(thread_id) = list_picker::selected_resumable_thread_id(app)
                            {
                                restore_thread_by_id(thread_id.as_str(), app, agent_slot)?;
                                app.dismiss_overlay();
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
                    app.dismiss_overlay();
                    if !(resume_pending_shell_approval_after_full_access(
                        app,
                        agent_slot,
                        runtime_port,
                    )
                    .await?)
                    {
                        app.push_notice(format!("Permission mode: {label}."));
                    }
                }
            }
            _ => {}
        },
    }
    Ok(false)
}

async fn request_maintenance(
    app: &mut TuiApp,
    runtime_port: Option<&dyn RuntimeClientPort>,
    command: RuntimeMaintenanceCommand,
) -> anyhow::Result<()> {
    if let Some(runtime_port) = runtime_port {
        runtime_port
            .send(RuntimeCommand::Maintenance(command))
            .await?;
    } else {
        match command {
            RuntimeMaintenanceCommand::Rebuild => super::runtime::start_rebuild_task(app),
            RuntimeMaintenanceCommand::RefreshModelCatalog(provider) => {
                super::runtime::start_model_catalog_task(app, provider)
            }
            RuntimeMaintenanceCommand::Compact => {
                app.push_notice("Compaction requires an active runtime client.")
            }
        }
    }
    Ok(())
}

async fn resume_pending_shell_approval_after_full_access(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<bool> {
    if app.permission_mode != PermissionMode::FullAccess
        || !app.active_pending_interaction().is_some_and(|interaction| {
            interaction.kind == ActivePendingInteractionKind::ShellApproval
        })
        || app.is_busy()
    {
        return Ok(false);
    }

    if let Some(runtime_port) = runtime_port {
        runtime_port
            .send(RuntimeCommand::Input(
                crate::runtime_control::InputControlRequest::AnswerShellApproval {
                    decision: ShellApprovalDecision::Once,
                },
            ))
            .await?;
    } else if agent_slot.is_some() {
        input_control::answer_shell_approval(app, agent_slot, ShellApprovalDecision::Once);
    } else {
        app.push_notice("Permission mode: full-access. Approval is still preparing.");
    }
    Ok(true)
}
