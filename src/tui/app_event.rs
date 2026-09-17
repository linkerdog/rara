use super::selection::ScreenPosition;
use super::state::{HelpTab, Overlay, StatusTab};

#[derive(Debug, Clone)]
pub enum AppEvent {
    Noop,
    /// Reserved for protocol/UI-control callers that should request overlays
    /// without reaching into TUI state directly (docs/todo.md).
    #[allow(dead_code)] // Reserved for protocol/UI-control callers (docs/todo.md)
    OpenOverlay(Overlay),
    CloseOverlay,
    SubmitComposer,
    InsertNewline,
    InputChar(char),
    Backspace,
    DeleteForward,
    MoveCursorLeft,
    MoveCursorRight,
    MoveCursorHome,
    MoveCursorEnd,
    MoveCursorUp,
    MoveCursorDown,
    NavigateInputHistory(i32),
    ScrollTranscript(i32),
    StartTranscriptSelection(ScreenPosition),
    DragTranscriptSelection(ScreenPosition),
    FinishTranscriptSelection(ScreenPosition),
    ScrollContext(i32),
    MoveCommandSelection(i32),
    MoveApprovalSelection(i32),
    MovePermissionSelection(i32),
    SetPermissionSelection(usize),
    MoveSkillsSelection(i32),
    /// Generic list-picker move/set events — used by Overlay::ListPicker.
    MoveListPickerSelection(i32),
    SetListPickerSelection(usize),
    SelectPendingOption(usize),
    /// Reserved for legacy setup picker navigation until provider selection is
    /// fully consolidated into list-picker events (docs/todo.md).
    #[allow(dead_code)] // Reserved for legacy setup picker navigation (docs/todo.md)
    CycleModelSelection,
    SaveBaseUrlInput,
    SaveApiKeyInput,
    SaveModelNameInput,
    SaveOpenAiProfileLabelInput,
    /// Reserved for direct OpenAI-compatible profile creation shortcuts once
    /// profile actions move out of overlay-only dispatch (docs/todo.md).
    #[allow(dead_code)] // Reserved for direct profile creation shortcuts (docs/todo.md)
    CreateOpenAiProfile,
    /// Reserved for direct OpenAI-compatible profile edit shortcuts once
    /// profile actions move out of overlay-only dispatch (docs/todo.md).
    #[allow(dead_code)] // Reserved for direct profile edit shortcuts (docs/todo.md)
    EditOpenAiProfile,
    /// Reserved for direct OpenAI-compatible profile deletion shortcuts once
    /// profile actions move out of overlay-only dispatch (docs/todo.md).
    #[allow(dead_code)] // Reserved for direct profile deletion shortcuts (docs/todo.md)
    DeleteOpenAiProfile,
    SelectHelpTab(HelpTab),
    SelectStatusTab(StatusTab),
    ApplyOverlaySelection,
    CycleResumeSort,
    ClearResumeSearch,
    CancelRunningTask,
    ClearComposer,
    ToggleSidebar,
    ToggleThinking,
}
