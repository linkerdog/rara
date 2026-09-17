use crate::tui::state::{LocalCommand, LocalCommandKind, TuiApp};

pub(crate) fn command_unavailable_reason(
    app: &TuiApp,
    command: &LocalCommand,
) -> Option<&'static str> {
    if !app.is_busy() {
        return None;
    }
    let allowed = match command.kind {
        LocalCommandKind::Help
        | LocalCommandKind::Status
        | LocalCommandKind::Context
        | LocalCommandKind::Permissions
        | LocalCommandKind::Skills
        | LocalCommandKind::Mcp
        | LocalCommandKind::Quit => true,
        LocalCommandKind::Tasks | LocalCommandKind::Goal => command.arg.is_none(),
        LocalCommandKind::Approval
        | LocalCommandKind::Clear
        | LocalCommandKind::Compact
        | LocalCommandKind::Connect
        | LocalCommandKind::Model
        | LocalCommandKind::NowledgeMem
        | LocalCommandKind::Plan
        | LocalCommandKind::Resume
        | LocalCommandKind::Review => false,
    };
    (!allowed).then_some("Unavailable while a task is running. Wait or cancel it first.")
}
