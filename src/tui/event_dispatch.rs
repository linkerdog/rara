use std::sync::Arc;

use rara_provider_catalog::ModelCatalogProvider;

use super::app_event::AppEvent;
use super::command::{palette_command_by_index, palette_commands};
use super::input_control;
#[allow(unused_imports)]
use super::list_picker;
use super::model_search::matching_model_presets;
use super::provider_flow::{
    open_provider_family_overlay, should_open_codex_auth_guide,
    sync_codex_credential_from_auth_store,
};
use super::runtime::start_oauth_task;
use super::runtime_port::{RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand};
use super::session_restore::request_restore_thread;
use super::state::{
    ActivePendingInteractionKind, ApiKeyTarget, ListPickerKind, OpenAiModelPickerAction, Overlay,
    ProviderFamily, QuitShortcutAction, QuitShortcutKey, TuiApp,
};
use super::submit::{apply_openai_model_picker_action, handle_submit, handle_submit_with_port};
use super::terminal_ui::is_ssh_session;
use crate::agent::Agent;
use crate::config::DEFAULT_CODEX_BASE_URL;
use crate::oauth::{OAuthManager, SavedCodexAuthMode};
use crate::runtime_control::SessionControlRequest;
use crate::tui::state::NoticeLevel;

mod credentials;
mod maintenance;
mod model_selection;

use maintenance::request_maintenance;
use model_selection::apply_model_selection;

#[cfg(test)]
pub(crate) async fn dispatch_event(
    event: AppEvent,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
) -> anyhow::Result<bool> {
    let result = dispatch_event_inner(event, app, agent_slot, oauth_manager, None).await;
    app.refresh_file_mentions();
    result
}

pub(crate) async fn dispatch_event_with_runtime(
    event: AppEvent,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
    runtime_port: &dyn RuntimeClientPort,
) -> anyhow::Result<bool> {
    let result =
        dispatch_event_inner(event, app, agent_slot, oauth_manager, Some(runtime_port)).await;
    app.refresh_file_mentions();
    result
}

async fn dispatch_event_inner(
    event: AppEvent,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    oauth_manager: &Arc<OAuthManager>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<bool> {
    let event = if let AppEvent::QuitShortcut(key) = event {
        match app.quit_shortcut.press(key, std::time::Instant::now()) {
            QuitShortcutAction::Confirmed => return Ok(true),
            QuitShortcutAction::Armed => match key {
                QuitShortcutKey::CtrlC if app.is_busy() => AppEvent::CancelRunningTask,
                QuitShortcutKey::CtrlC => AppEvent::ClearComposer,
                QuitShortcutKey::CtrlD => AppEvent::Noop,
            },
        }
    } else {
        app.quit_shortcut.clear();
        event
    };
    let discarding_palette = matches!(&event, AppEvent::CloseOverlay)
        && matches!(app.overlay, Some(Overlay::CommandPalette));
    if app.composer_input_is_active() && !discarding_palette {
        app.flush_composer_paste();
    }
    match event {
        AppEvent::QuitShortcut(_) => {
            anyhow::bail!("quit shortcut reached dispatch without resolution");
        }
        AppEvent::Goal(action) => {
            super::runtime::apply_goal_dialog_action(action, app, agent_slot, runtime_port).await;
        }
        AppEvent::FileMention(action) => app.apply_file_mention(action),
        AppEvent::Noop => {}
        AppEvent::OpenOverlay(overlay) => app.open_overlay(overlay),
        AppEvent::CloseOverlay => {
            if super::session_restore::cancel_restore(app, agent_slot) {
                app.push_notice(NoticeLevel::Info, "Thread restore cancelled.");
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
            if super::session_restore::cancel_restore(app, agent_slot) {
                app.push_notice(NoticeLevel::Info, "Thread restore cancelled.");
                return Ok(false);
            }
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
            app.clear_composer();
            app.reset_input_history_navigation();
            app.sync_command_palette_with_input();
        }
        AppEvent::ToggleSidebar => {
            app.sidebar_visible = !app.sidebar_visible;
        }
        AppEvent::ToggleThinking => {
            app.thinking_collapsed = !app.thinking_collapsed;
        }
        AppEvent::SubmitComposer => {
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
            if app.composer_input_is_active() && app.bottom_pane.input.is_empty() {
                app.transcript_scroll.follow_tail();
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
        AppEvent::PromptHistory(action) => app.apply_history_action(action),
        AppEvent::ScrollTranscript(delta) => super::render::scroll_transcript(app, delta),
        AppEvent::StartTranscriptSelection(position) => {
            app.transcript_selection.start(position);
        }
        AppEvent::DragTranscriptSelection(position) => {
            app.transcript_selection.drag(position);
        }
        AppEvent::FinishTranscriptSelection(position) => {
            if let Some(text) = app.transcript_selection.finish(position) {
                let notice = app
                    .clipboard
                    .get_or_insert_with(super::clipboard::Clipboard::from_environment)
                    .request(text);
                app.push_notice(notice.level, notice.message);
            }
        }
        AppEvent::NavigateOverlay(navigation) => super::render::navigate_overlay(app, navigation),
        AppEvent::MoveCommandSelection(delta) => {
            if matches!(app.overlay, Some(Overlay::ModelSearch)) {
                let count = matching_model_presets(app).len();
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
        AppEvent::MoveListPickerSelection(delta) => {
            let Some(Overlay::ListPicker(kind)) = app.overlay else {
                return Ok(false);
            };
            if kind == ListPickerKind::Resume {
                app.move_resume_selection(delta);
                return Ok(false);
            }
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
        AppEvent::ScrollApprovalDetails(direction) => {
            if let Some(request_id) = app
                .active_pending_interaction()
                .filter(|pending| pending.kind == ActivePendingInteractionKind::ShellApproval)
                .and_then(|pending| pending._snapshot.approval.as_ref())
                .map(|approval| approval.tool_use_id.clone())
            {
                app.bottom_pane
                    .approval_details
                    .navigate(&request_id, direction);
            }
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
                            app.push_notice(NoticeLevel::Warning, "Invalid plan approval option.");
                        }
                    }
                    ActivePendingInteractionKind::ShellApproval => {
                        if let Some(selection) =
                            input_control::shell_approval_decision_for_index(idx)
                        {
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
                        } else {
                            app.push_notice(NoticeLevel::Warning, "Invalid shell approval option.");
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
                                    NoticeLevel::Info,
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
                app.push_notice(
                    NoticeLevel::Info,
                    "Wait for the current task before saving the base URL.",
                );
            } else {
                let value = app.base_url_input.trim();
                app.config
                    .set_base_url((!value.is_empty()).then(|| value.to_string()));
                app.config_manager.save(&app.config)?;
                app.push_notice(
                    NoticeLevel::Info,
                    format!(
                        "Saved base URL: {}",
                        app.config.base_url.as_deref().unwrap_or("unset")
                    ),
                );
                if app.openai_setup_steps.is_empty() {
                    app.dismiss_overlay();
                } else {
                    app.advance_openai_profile_setup();
                }
            }
        }
        AppEvent::SaveApiKeyInput => {
            credentials::save_api_key(app, agent_slot, runtime_port).await?;
        }
        AppEvent::SaveModelNameInput => {
            if app.is_busy() {
                app.push_notice(
                    NoticeLevel::Info,
                    "Wait for the current task before saving the model name.",
                );
            } else {
                let value = app.model_name_input.trim();
                app.config
                    .set_model((!value.is_empty()).then(|| value.to_string()));
                app.config_manager.save(&app.config)?;
                app.push_notice(
                    NoticeLevel::Info,
                    format!(
                        "Saved model name: {}",
                        app.config.model.as_deref().unwrap_or("unset")
                    ),
                );
                if app.openai_setup_steps.is_empty() {
                    app.dismiss_overlay();
                } else {
                    app.advance_openai_profile_setup();
                }
            }
        }
        AppEvent::SaveOpenAiProfileLabelInput => {
            if app.is_busy() {
                app.push_notice(
                    NoticeLevel::Info,
                    "Wait for the current task before creating a profile.",
                );
            } else if app.selected_provider_family() != ProviderFamily::OpenAiCompatible {
                app.push_notice(
                    NoticeLevel::Warning,
                    "OpenAI-compatible profiles are only available in that provider family.",
                );
            } else {
                let label = app.openai_profile_label_input.trim();
                if label.is_empty() {
                    app.push_notice(
                        NoticeLevel::Info,
                        "Enter a profile label or press Esc to go back.",
                    );
                } else if let Some(kind) = app
                    .openai_profile_label_kind
                    .or_else(|| app.selected_openai_profile_kind())
                {
                    let profile_id = app.next_openai_profile_id(kind, label);
                    app.config.select_openai_profile(profile_id, label, kind);
                    app.config_manager.save(&app.config)?;
                    app.push_notice(
                        NoticeLevel::Info,
                        format!("Created endpoint profile: {label}"),
                    );
                    app.openai_profile_label_kind = None;
                    app.begin_created_openai_profile_setup();
                }
            }
        }
        AppEvent::CreateOpenAiProfile => {
            if app.is_busy() {
                app.push_notice(
                    NoticeLevel::Info,
                    "Wait for the current task before creating a profile.",
                );
            } else if app.selected_provider_family() == ProviderFamily::OpenAiCompatible {
                app.begin_openai_profile_setup();
            }
        }
        AppEvent::EditOpenAiProfile => {
            if app.is_busy() {
                app.push_notice(
                    NoticeLevel::Info,
                    "Wait for the current task before editing a profile.",
                );
            } else if app.selected_provider_family() == ProviderFamily::OpenAiCompatible
                && app.select_openai_model_picker_profile().is_some()
            {
                app.config_manager.save(&app.config)?;
                app.begin_edit_openai_profile_setup();
            }
        }
        AppEvent::DeleteOpenAiProfile => {
            if app.is_busy() {
                app.push_notice(
                    NoticeLevel::Info,
                    "Wait for the current task before deleting a profile.",
                );
            } else if app.selected_provider_family() == ProviderFamily::OpenAiCompatible {
                apply_openai_model_picker_action(
                    app,
                    OpenAiModelPickerAction::DeleteProfile,
                    runtime_port,
                    agent_slot.as_ref().and_then(Agent::agent_tree_control),
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
        AppEvent::ToggleResumeScope => app.toggle_resume_scope(),
        AppEvent::RefreshResume => app.refresh_recent_threads_for_resume_picker(),
        AppEvent::ResumePageUp => {
            app.move_resume_selection(-(app.resume_query.page_items.max(1) as i32))
        }
        AppEvent::ResumePageDown => {
            app.move_resume_selection(app.resume_query.page_items.max(1) as i32)
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
                if let Some(preset) = matching_model_presets(app)
                    .get(app.model_search_idx)
                    .cloned()
                {
                    app.dismiss_overlay();
                    apply_model_selection(
                        preset,
                        app,
                        agent_slot,
                        oauth_manager.as_ref(),
                        runtime_port,
                    )
                    .await?;
                }
            }
            Some(Overlay::CommandPalette) => {
                let query = app.command_query();
                if let Some(spec) = palette_command_by_index(app, query, app.command_palette_idx) {
                    // Save the command text before close_overlay, which clears
                    // the composer input for CommandPalette to prevent immediate
                    // re-open via sync_command_palette_with_input.
                    let invocation = format!("/{}", spec.name);
                    app.dismiss_overlay();
                    app.bottom_pane.input = invocation;
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
                    app.push_notice(
                        NoticeLevel::Info,
                        "Wait for the current task before saving the base URL.",
                    );
                } else {
                    let value = app.base_url_input.trim();
                    app.config
                        .set_base_url((!value.is_empty()).then(|| value.to_string()));
                    app.config_manager.save(&app.config)?;
                    app.push_notice(
                        NoticeLevel::Info,
                        format!(
                            "Saved base URL: {}",
                            app.config.base_url.as_deref().unwrap_or("unset")
                        ),
                    );
                    app.dismiss_overlay();
                }
            }
            Some(Overlay::ListPicker(kind)) => {
                if app.is_busy() {
                    app.push_notice(
                        NoticeLevel::Info,
                        "A task is already running. Wait for it to finish.",
                    );
                } else {
                    match kind {
                        ListPickerKind::Provider => {
                            open_provider_family_overlay(app);
                        }
                        ListPickerKind::Model => {
                            if app.selected_provider_family() == ProviderFamily::Codex
                                && let Err(error) = sync_codex_credential_from_auth_store(
                                    app,
                                    oauth_manager.as_ref(),
                                )
                            {
                                log::warn!("Could not load saved credential: {error:#}");
                                app.push_notice(
                                    NoticeLevel::Error,
                                    format!("Could not load saved credential: {error:#}"),
                                );
                                return Ok(false);
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
                                        agent_slot,
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
                                    apply_openai_model_picker_action(
                                        app,
                                        action,
                                        runtime_port,
                                        agent_slot.as_ref().and_then(Agent::agent_tree_control),
                                    )
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
                                        agent_slot,
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
                                        agent_slot,
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
                                        agent_slot,
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
                                    agent_slot,
                                    runtime_port,
                                    RuntimeMaintenanceCommand::Rebuild,
                                )
                                .await?;
                            }
                        }
                        ListPickerKind::UnifiedModel => {
                            if let Some(preset) = app
                                .all_unified_model_presets()
                                .get(app.model_picker_idx)
                                .cloned()
                            {
                                apply_model_selection(
                                    preset,
                                    app,
                                    agent_slot,
                                    oauth_manager.as_ref(),
                                    runtime_port,
                                )
                                .await?;
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
                            0 => app.push_notice(
                                NoticeLevel::Warning,
                                "Browser login unavailable in SSH/headless.",
                            ),
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
                                app.push_notice(
                                    NoticeLevel::Info,
                                    if removed {
                                        "Cleared saved credential."
                                    } else {
                                        "No saved credential present."
                                    },
                                );
                                if app.config.provider == "codex" {
                                    request_maintenance(
                                        app,
                                        agent_slot,
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
                                agent_slot,
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
                            app.push_notice(
                                NoticeLevel::Info,
                                format!(
                                    "Saved Nowledge Mem {} configuration. Rebuilding runtime.",
                                    mode_label
                                ),
                            );
                            app.dismiss_overlay();
                            request_maintenance(
                                app,
                                agent_slot,
                                runtime_port,
                                RuntimeMaintenanceCommand::Rebuild,
                            )
                            .await?;
                        }
                        ListPickerKind::Resume => {
                            if let Some(thread_id) = list_picker::selected_resumable_thread_id(app)
                                && let Err(error) =
                                    request_restore_thread(thread_id.as_str(), app, agent_slot)
                            {
                                log::warn!("Could not resume thread {thread_id}: {error:#}");
                                app.push_notice(
                                    NoticeLevel::Error,
                                    format!("Could not resume thread {thread_id}: {error:#}"),
                                );
                                return Ok(false);
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
                                app.push_notice(
                                    NoticeLevel::Info,
                                    format!("Selected endpoint profile: {label}"),
                                );
                                app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
                            }
                        }
                    }
                }
            }
            Some(Overlay::PermissionPicker) => {
                if let Some(preset) =
                    crate::tui::permission_policy::PERMISSION_PRESETS.get(app.permission_picker_idx)
                {
                    if let Some(runtime_port) = runtime_port {
                        runtime_port
                            .send(RuntimeCommand::SetPermissionMode(preset.mode))
                            .await?;
                        app.push_notice(
                            NoticeLevel::Info,
                            format!("Permission change requested: {}.", preset.mode.label()),
                        );
                    } else {
                        super::runtime::request_permission_mode(app, agent_slot, preset.mode);
                    }
                    app.dismiss_overlay();
                }
            }
            _ => {}
        },
    }
    Ok(false)
}
