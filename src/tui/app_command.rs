// Agent operation commands — decoupled from UI event dispatch.
//
// AppEvent represents what happened in the UI; AppCommand represents
// what the agent should do in response.  This split matches Codex's
// pattern and leaves room for future multi-thread / team-mode routing.
//
// Dispatch produces zero or more commands; the event loop then enqueues
// them against the agent slot.
use crate::runtime_control::ShellApprovalDecision;

#[derive(Debug, Clone)]
pub(crate) enum AppCommand {
    /// Placeholder — no agent action needed.
    Noop,

    /// Graceful exit requested (e.g. `/quit`).
    Quit,

    /// Submit the current composer text as a user turn.
    SubmitInput,

    /// Interrupt the running agent task.
    Interrupt,

    /// Approve the pending plan and enter execution mode.
    ApprovePlan,

    /// Keep planning (decline plan approval).
    ContinuePlanning,

    /// Approve a pending shell command.
    ApproveShell(ShellApprovalDecision),

    /// Answer a structured question or request-input interaction.
    AnswerQuestion(String),

    /// Apply a new permission mode.
    SetPermissionMode(crate::tui::state::PermissionMode),

    /// Reload configuration from disk and restart the backend.
    ReloadConfig,

    /// The backend needs rebuilding (model or profile changed).
    NeedsRebuild,

    /// Start the OAuth / login flow.
    StartOAuth,
}

/// Map an AppEvent to the corresponding agent-level commands.
///
/// This is a pure function — it reads only the event, not app state.
/// As dispatch is gradually refactored to emit commands instead of
/// calling agent functions directly, more cases will be added here.
pub(crate) fn commands_for_event(event: &super::app_event::AppEvent) -> Vec<AppCommand> {
    let mut cmds = Vec::new();
    match event {
        super::app_event::AppEvent::SubmitComposer => cmds.push(AppCommand::SubmitInput),
        super::app_event::AppEvent::CancelRunningTask => cmds.push(AppCommand::Interrupt),
        _ => {}
    }
    cmds
}
