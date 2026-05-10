use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app_event::AppEvent;
use super::state::{HelpTab, Overlay, StatusTab, TuiApp};

pub(crate) fn map_key_to_event(key: KeyEvent, app: &TuiApp) -> AppEvent {
    let code = key.code;
    let modifiers = key.modifiers;
    match app.overlay {
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
        Some(Overlay::CommandPalette) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Up | KeyCode::Char('k') => AppEvent::MoveCommandSelection(-1),
            KeyCode::Down | KeyCode::Char('j') => AppEvent::MoveCommandSelection(1),
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
            KeyCode::Up | KeyCode::Char('k') => AppEvent::ScrollContext(-1),
            KeyCode::Down | KeyCode::Char('j') => AppEvent::ScrollContext(1),
            KeyCode::PageUp => AppEvent::ScrollContext(-5),
            KeyCode::PageDown => AppEvent::ScrollContext(5),
            _ => AppEvent::Noop,
        },
        Some(Overlay::SkillsPicker) => match code {
            KeyCode::Esc => AppEvent::CloseOverlay,
            KeyCode::Up | KeyCode::Char('k') => AppEvent::MoveSkillsSelection(-1),
            KeyCode::Down | KeyCode::Char('j') => AppEvent::MoveSkillsSelection(1),
            KeyCode::Char(' ') => AppEvent::ToggleSkillSelection,
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
        Some(Overlay::ApiKeyEditor) => match code {
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

            match (code, modifiers) {
                (KeyCode::Esc, _) if app.is_busy() => AppEvent::CancelRunningTask,
                (KeyCode::Char('c'), KeyModifiers::CONTROL) if app.is_busy() => {
                    AppEvent::CancelRunningTask
                }
                (KeyCode::Esc, _) => AppEvent::Noop,
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => AppEvent::ClearComposer,
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
                (KeyCode::Char('k'), _) if app.bottom_pane.input.is_empty() => {
                    AppEvent::ScrollTranscript(-1)
                }
                (KeyCode::Char('j'), _) if app.bottom_pane.input.is_empty() => {
                    AppEvent::ScrollTranscript(1)
                }
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
                (KeyCode::Backspace, _) => AppEvent::Backspace,
                (KeyCode::Delete, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    AppEvent::DeleteForward
                }
                (KeyCode::Char('b'), KeyModifiers::CONTROL) => AppEvent::ToggleSidebar,
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
