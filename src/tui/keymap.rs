use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app_event::AppEvent;
use super::prompt_history::HistoryAction;
use super::state::{
    ApprovalDetailNavigation, HelpTab, Overlay, OverlayNavigation, QuitShortcutKey, StatusTab,
    TuiApp,
};

pub(crate) fn map_key_to_event(key: KeyEvent, app: &TuiApp) -> AppEvent {
    let code = key.code;
    let modifiers = key.modifiers;
    // Control shortcuts must not fall through to an overlay's printable input.
    if modifiers == KeyModifiers::CONTROL {
        if app.overlay == Some(Overlay::ListPicker(super::state::ListPickerKind::Resume)) {
            match code {
                KeyCode::Char('s') => return AppEvent::CycleResumeSort,
                KeyCode::Char('r') => return AppEvent::RefreshResume,
                KeyCode::Char('d') => return AppEvent::DeleteForward,
                _ => {}
            }
        }
        match code {
            KeyCode::Char('r') => {
                return if app.overlay == Some(Overlay::HistorySearch) {
                    AppEvent::PromptHistory(HistoryAction::Older)
                } else if matches!(app.overlay, None | Some(Overlay::CommandPalette))
                    && app.active_pending_interaction().is_none()
                {
                    AppEvent::PromptHistory(HistoryAction::Open)
                } else {
                    AppEvent::Noop
                };
            }
            KeyCode::Char('c') => {
                return if app.overlay.is_some() {
                    AppEvent::CloseOverlay
                } else {
                    AppEvent::QuitShortcut(QuitShortcutKey::CtrlC)
                };
            }
            KeyCode::Char('d') => {
                return match app.overlay {
                    None if app.bottom_pane.input.is_empty() => {
                        AppEvent::QuitShortcut(QuitShortcutKey::CtrlD)
                    }
                    None
                    | Some(
                        Overlay::CommandPalette
                        | Overlay::HistorySearch
                        | Overlay::ModelSearch
                        | Overlay::BaseUrlEditor
                        | Overlay::ApiKeyEditor(_)
                        | Overlay::ModelNameEditor
                        | Overlay::OpenAiProfileLabelEditor,
                    ) => AppEvent::DeleteForward,
                    Some(Overlay::Goal)
                        if matches!(
                            app.goal_ui.dialog,
                            Some(super::goal_ui::GoalDialog::Edit(_))
                        ) =>
                    {
                        AppEvent::DeleteForward
                    }
                    Some(
                        Overlay::Goal
                        | Overlay::Help(_)
                        | Overlay::Status(_)
                        | Overlay::Context
                        | Overlay::SkillsPicker
                        | Overlay::ListPicker(_)
                        | Overlay::PermissionPicker,
                    ) => AppEvent::Noop,
                };
            }
            _ => {}
        }
    }
    if matches!(
        app.overlay,
        Some(Overlay::Help(_) | Overlay::Status(_) | Overlay::Context)
    ) {
        let navigation = match code {
            KeyCode::Up | KeyCode::Char('k') => Some(OverlayNavigation::Rows(-1)),
            KeyCode::Down | KeyCode::Char('j') => Some(OverlayNavigation::Rows(1)),
            KeyCode::PageUp => Some(OverlayNavigation::PageUp),
            KeyCode::PageDown => Some(OverlayNavigation::PageDown),
            KeyCode::Home => Some(OverlayNavigation::Start),
            KeyCode::End => Some(OverlayNavigation::End),
            _ => None,
        };
        if let Some(navigation) = navigation {
            return AppEvent::NavigateOverlay(navigation);
        }
    }
    match app.overlay {
        Some(Overlay::HistorySearch) => match (code, modifiers) {
            (KeyCode::Esc, _) => AppEvent::CloseOverlay,
            (KeyCode::Enter, _) => AppEvent::PromptHistory(HistoryAction::Accept),
            (KeyCode::Up, _) => AppEvent::PromptHistory(HistoryAction::Older),
            (KeyCode::Down, _) | (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                AppEvent::PromptHistory(HistoryAction::Newer)
            }
            (KeyCode::Backspace, _) => AppEvent::Backspace,
            (KeyCode::Delete, _) => AppEvent::DeleteForward,
            (KeyCode::Left, _) => AppEvent::MoveCursorLeft,
            (KeyCode::Right, _) => AppEvent::MoveCursorRight,
            (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                AppEvent::MoveCursorHome
            }
            (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                AppEvent::MoveCursorEnd
            }
            (KeyCode::Char(c), modifiers)
                if !modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                AppEvent::InputChar(c)
            }
            _ => AppEvent::Noop,
        },
        Some(Overlay::Goal) => super::goal_ui::key_event(app, code),
        Some(Overlay::Help(_)) => match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => AppEvent::CloseOverlay,
            KeyEvent {
                code: KeyCode::Char('1'),
                ..
            } => AppEvent::SelectHelpTab(HelpTab::General),
            KeyEvent {
                code: KeyCode::Char('2'),
                ..
            } => AppEvent::SelectHelpTab(HelpTab::Commands),
            KeyEvent {
                code: KeyCode::Char('3'),
                ..
            } => AppEvent::SelectHelpTab(HelpTab::Runtime),
            _ => AppEvent::Noop,
        },
        Some(Overlay::CommandPalette | Overlay::ModelSearch) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Up => AppEvent::MoveCommandSelection(-1),
            KeyCode::Down => AppEvent::MoveCommandSelection(1),
            KeyCode::Enter => AppEvent::ApplyOverlaySelection,
            KeyCode::Left => AppEvent::MoveCursorLeft,
            KeyCode::Right => AppEvent::MoveCursorRight,
            KeyCode::Home => AppEvent::MoveCursorHome,
            KeyCode::End => AppEvent::MoveCursorEnd,
            KeyCode::Backspace => AppEvent::Backspace,
            KeyCode::Delete => AppEvent::DeleteForward,
            KeyCode::Char(c) => AppEvent::InputChar(c),
            _ => AppEvent::Noop,
        },
        Some(Overlay::Status(tab)) => match code {
            KeyCode::Esc | KeyCode::Enter => AppEvent::CloseOverlay,
            KeyCode::Char('1') => AppEvent::SelectStatusTab(StatusTab::Overview),
            KeyCode::Char('2') => AppEvent::SelectStatusTab(StatusTab::Config),
            KeyCode::Char('3') => AppEvent::SelectStatusTab(StatusTab::Context),
            KeyCode::Right | KeyCode::Tab => AppEvent::SelectStatusTab(next_status_tab(tab)),
            KeyCode::Left | KeyCode::BackTab => AppEvent::SelectStatusTab(prev_status_tab(tab)),
            _ => AppEvent::Noop,
        },
        Some(Overlay::Context) => match code {
            KeyCode::Esc | KeyCode::Enter => AppEvent::CloseOverlay,
            _ => AppEvent::Noop,
        },
        Some(Overlay::SkillsPicker) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Up | KeyCode::Char('k') => AppEvent::MoveSkillsSelection(-1),
            KeyCode::Down | KeyCode::Char('j') => AppEvent::MoveSkillsSelection(1),
            KeyCode::Enter => AppEvent::CloseOverlay,
            _ => AppEvent::Noop,
        },
        Some(Overlay::ListPicker(kind)) => super::list_picker::list_picker_key_event(kind, code),
        Some(Overlay::PermissionPicker) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Up | KeyCode::Char('k') => AppEvent::MovePermissionSelection(-1),
            KeyCode::Down | KeyCode::Char('j') => AppEvent::MovePermissionSelection(1),
            KeyCode::Char('1') => AppEvent::SetPermissionSelection(0),
            KeyCode::Char('2') => AppEvent::SetPermissionSelection(1),
            KeyCode::Char('3') => AppEvent::SetPermissionSelection(2),
            KeyCode::Char('4') => AppEvent::SetPermissionSelection(3),
            KeyCode::Enter => AppEvent::ApplyOverlaySelection,
            _ => AppEvent::Noop,
        },
        Some(Overlay::BaseUrlEditor) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Enter => AppEvent::SaveBaseUrlInput,
            KeyCode::Left => AppEvent::MoveCursorLeft,
            KeyCode::Right => AppEvent::MoveCursorRight,
            KeyCode::Home => AppEvent::MoveCursorHome,
            KeyCode::End => AppEvent::MoveCursorEnd,
            KeyCode::Backspace => AppEvent::Backspace,
            KeyCode::Delete => AppEvent::DeleteForward,
            KeyCode::Char(c) => AppEvent::InputChar(c),
            _ => AppEvent::Noop,
        },
        Some(Overlay::ApiKeyEditor(_)) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Enter => AppEvent::SaveApiKeyInput,
            KeyCode::Left => AppEvent::MoveCursorLeft,
            KeyCode::Right => AppEvent::MoveCursorRight,
            KeyCode::Home => AppEvent::MoveCursorHome,
            KeyCode::End => AppEvent::MoveCursorEnd,
            KeyCode::Backspace => AppEvent::Backspace,
            KeyCode::Delete => AppEvent::DeleteForward,
            KeyCode::Char(c) => AppEvent::InputChar(c),
            _ => AppEvent::Noop,
        },
        Some(Overlay::ModelNameEditor) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Enter => AppEvent::SaveModelNameInput,
            KeyCode::Left => AppEvent::MoveCursorLeft,
            KeyCode::Right => AppEvent::MoveCursorRight,
            KeyCode::Home => AppEvent::MoveCursorHome,
            KeyCode::End => AppEvent::MoveCursorEnd,
            KeyCode::Backspace => AppEvent::Backspace,
            KeyCode::Delete => AppEvent::DeleteForward,
            KeyCode::Char(c) => AppEvent::InputChar(c),
            _ => AppEvent::Noop,
        },
        Some(Overlay::OpenAiProfileLabelEditor) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Enter => AppEvent::SaveOpenAiProfileLabelInput,
            KeyCode::Left => AppEvent::MoveCursorLeft,
            KeyCode::Right => AppEvent::MoveCursorRight,
            KeyCode::Home => AppEvent::MoveCursorHome,
            KeyCode::End => AppEvent::MoveCursorEnd,
            KeyCode::Backspace => AppEvent::Backspace,
            KeyCode::Delete => AppEvent::DeleteForward,
            KeyCode::Char(c) => AppEvent::InputChar(c),
            _ => AppEvent::Noop,
        },
        None => {
            if app.bottom_pane.input.is_empty()
                && let Some(index) = pending_shortcut_index(code, app)
            {
                return AppEvent::SelectPendingOption(index);
            }
            // Approval cards keep their horizontal visual order in keyboard navigation.
            // Navigation requires an empty composer so regular typing still works.
            if app.active_pending_interaction().is_some_and(|interaction| {
                matches!(
                    interaction.kind,
                    super::state::ActivePendingInteractionKind::ShellApproval
                        | super::state::ActivePendingInteractionKind::PlanApproval
                )
            }) {
                if code == KeyCode::Enter
                    && modifiers.is_empty()
                    && app.bottom_pane.input.is_empty()
                {
                    return AppEvent::SelectPendingOption(app.approval_picker_idx);
                }
                if app.active_pending_interaction().is_some_and(|interaction| {
                    interaction.kind == super::state::ActivePendingInteractionKind::ShellApproval
                }) && code == KeyCode::Esc
                    && modifiers.is_empty()
                {
                    return AppEvent::SelectPendingOption(3);
                }
                if app.bottom_pane.input.is_empty() {
                    if app.active_pending_interaction().is_some_and(|interaction| {
                        interaction.kind
                            == super::state::ActivePendingInteractionKind::ShellApproval
                    }) && modifiers.is_empty()
                    {
                        let direction = match code {
                            KeyCode::PageUp => Some(ApprovalDetailNavigation::PageUp),
                            KeyCode::PageDown => Some(ApprovalDetailNavigation::PageDown),
                            KeyCode::Home => Some(ApprovalDetailNavigation::Start),
                            KeyCode::End => Some(ApprovalDetailNavigation::End),
                            _ => None,
                        };
                        if let Some(direction) = direction {
                            return AppEvent::ScrollApprovalDetails(direction);
                        }
                    }
                    match (code, modifiers) {
                        (
                            KeyCode::Left | KeyCode::Char('h') | KeyCode::Up | KeyCode::Char('k'),
                            KeyModifiers::NONE,
                        ) => {
                            return AppEvent::MoveApprovalSelection(-1);
                        }
                        (
                            KeyCode::Right
                            | KeyCode::Char('l')
                            | KeyCode::Down
                            | KeyCode::Char('j'),
                            KeyModifiers::NONE,
                        ) => {
                            return AppEvent::MoveApprovalSelection(1);
                        }
                        _ => {}
                    }
                    if app.active_pending_interaction().is_some_and(|interaction| {
                        interaction.kind
                            == super::state::ActivePendingInteractionKind::ShellApproval
                    }) && let KeyCode::F(num @ 1..=4) = code
                    {
                        return AppEvent::SelectPendingOption((num - 1) as usize);
                    }
                }
            }

            match (code, modifiers) {
                (KeyCode::Esc, _) if app.is_busy() => AppEvent::CancelRunningTask,
                (KeyCode::Esc, _) => AppEvent::Noop,
                (KeyCode::Enter, KeyModifiers::SHIFT)
                | (KeyCode::Char('j'), KeyModifiers::CONTROL) => AppEvent::InsertNewline,
                (KeyCode::Enter, _) => AppEvent::SubmitComposer,
                (KeyCode::Left, _) => AppEvent::MoveCursorLeft,
                (KeyCode::Right, _) => AppEvent::MoveCursorRight,
                (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                    AppEvent::MoveCursorHome
                }
                (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                    AppEvent::MoveCursorEnd
                }
                (KeyCode::Up, _) if app.should_handle_input_history_navigation(-1) => {
                    AppEvent::NavigateInputHistory(-1)
                }
                (KeyCode::Up, _) if app.bottom_pane.input.is_empty() => {
                    AppEvent::ScrollTranscript(-1)
                }
                (KeyCode::Up, _) => AppEvent::MoveCursorUp,
                (KeyCode::Down, _) if app.should_handle_input_history_navigation(1) => {
                    AppEvent::NavigateInputHistory(1)
                }
                (KeyCode::Down, _) if app.bottom_pane.input.is_empty() => {
                    AppEvent::ScrollTranscript(1)
                }
                (KeyCode::Down, _) => AppEvent::MoveCursorDown,
                (KeyCode::PageUp, _) if app.bottom_pane.input.is_empty() => {
                    AppEvent::ScrollTranscript(-8)
                }
                (KeyCode::PageDown, _) if app.bottom_pane.input.is_empty() => {
                    AppEvent::ScrollTranscript(8)
                }
                (KeyCode::Char('1'), _)
                    if app.bottom_pane.input.is_empty()
                        && app.has_pending_planning_suggestion() =>
                {
                    AppEvent::SelectPendingOption(0)
                }
                (KeyCode::Char('2'), _)
                    if app.bottom_pane.input.is_empty()
                        && app.has_pending_planning_suggestion() =>
                {
                    AppEvent::SelectPendingOption(1)
                }
                (KeyCode::Char('1'), _)
                    if app.bottom_pane.input.is_empty() && app.has_pending_approval() =>
                {
                    AppEvent::SetPermissionSelection(0)
                }
                (KeyCode::Char('2'), _)
                    if app.bottom_pane.input.is_empty() && app.has_pending_approval() =>
                {
                    AppEvent::SetPermissionSelection(1)
                }
                (KeyCode::Char('3'), _)
                    if app.bottom_pane.input.is_empty() && app.has_pending_approval() =>
                {
                    AppEvent::SetPermissionSelection(2)
                }
                (KeyCode::Char('4'), _)
                    if app.bottom_pane.input.is_empty() && app.has_pending_approval() =>
                {
                    AppEvent::SetPermissionSelection(3)
                }
                (KeyCode::Backspace, _) => AppEvent::Backspace,
                (KeyCode::Delete, _) => AppEvent::DeleteForward,
                (KeyCode::Char('b'), KeyModifiers::CONTROL) => AppEvent::ToggleSidebar,
                (KeyCode::Char('t'), KeyModifiers::ALT) => AppEvent::ToggleThinking,
                (KeyCode::Char(c), _) => AppEvent::InputChar(c),
                _ => AppEvent::Noop,
            }
        }
    }
}

fn next_status_tab(tab: StatusTab) -> StatusTab {
    match tab {
        StatusTab::Overview => StatusTab::Config,
        StatusTab::Config => StatusTab::Context,
        StatusTab::Context => StatusTab::Overview,
    }
}

fn prev_status_tab(tab: StatusTab) -> StatusTab {
    match tab {
        StatusTab::Overview => StatusTab::Context,
        StatusTab::Config => StatusTab::Overview,
        StatusTab::Context => StatusTab::Config,
    }
}

fn pending_shortcut_index(code: KeyCode, app: &TuiApp) -> Option<usize> {
    let KeyCode::Char(ch) = code else {
        return None;
    };

    let index = match ch.to_digit(10) {
        Some(digit @ 1..=9) => digit as usize - 1,
        _ => return None,
    };

    (index < app.active_pending_option_count()).then_some(index)
}
